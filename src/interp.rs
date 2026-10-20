//! An interpreter for the IR. It runs the REPL, and it is the reference the native code is tested
//! against: every test program runs both ways (and at every optimization level), and the outputs must
//! be identical.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write;
use std::rc::Rc;
use std::time::Instant;

use crate::builtins::Builtin;
use crate::ir::{BinOp, FuncId, Inst, Module, Term, UnOp, V, ValueDef};

#[derive(Clone, Debug)]
pub enum Val {
    /// Integers, booleans (0/1) and unit (0).
    Int(i64),
    Str(Rc<str>),
    /// ADT values and tuples.
    Obj(Rc<Obj>),
    Arr(Rc<RefCell<Vec<Val>>>),
    Clo(Rc<Closure>),
}

#[derive(Debug)]
pub struct Obj {
    pub tag: u32,
    pub fields: Vec<Val>,
}

#[derive(Debug)]
pub struct Closure {
    pub func: FuncId,
    pub captures: Vec<Val>,
}

impl Val {
    fn int(&self) -> i64 {
        match self {
            Val::Int(n) => *n,
            other => panic!("expected an integer, got {other:?}"),
        }
    }

    fn str(&self) -> &str {
        match self {
            Val::Str(s) => s,
            other => panic!("expected a string, got {other:?}"),
        }
    }
}

/// Wraps to 63 bits, as the tagged machine representation does.
pub fn wrap63(n: i64) -> i64 {
    (n << 1) >> 1
}

/// A run-time failure: the program stopped with this message.
#[derive(Debug, Clone, PartialEq)]
pub struct Trap(pub String);

/// Structural equality: strings by content, constructors and tuples field by field, arrays and closures by
/// identity.
pub fn equal(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Int(x), Val::Int(y)) => x == y,
        (Val::Str(x), Val::Str(y)) => x == y,
        (Val::Obj(x), Val::Obj(y)) => {
            Rc::ptr_eq(x, y)
                || (x.tag == y.tag && x.fields.len() == y.fields.len() && x.fields.iter().zip(&y.fields).all(|(p, q)| equal(p, q)))
        }
        (Val::Arr(x), Val::Arr(y)) => Rc::ptr_eq(x, y),
        (Val::Clo(x), Val::Clo(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

pub struct Interp<'m> {
    m: &'m Module,
    out: &'m mut dyn Write,
    ctor0: Vec<Rc<Obj>>,
    statics: HashMap<FuncId, Rc<Closure>>,
    strings: Vec<Rc<str>>,
    start: Instant,
    /// Calls executed (for the REPL's `:stats`).
    pub calls: u64,
    pub steps: u64,
}

impl<'m> Interp<'m> {
    pub fn new(m: &'m Module, out: &'m mut dyn Write) -> Interp<'m> {
        Interp {
            m,
            out,
            ctor0: m.ctor0.iter().map(|c| Rc::new(Obj { tag: c.tag, fields: vec![] })).collect(),
            statics: HashMap::new(),
            strings: m.strings.iter().map(|s| Rc::from(s.as_str())).collect(),
            start: Instant::now(),
            calls: 0,
            steps: 0,
        }
    }

    pub fn run_main(&mut self) -> Result<Val, Trap> {
        let r = self.call(self.m.main, vec![]);
        let _ = self.out.flush();
        r
    }

    pub fn call(&mut self, f: FuncId, args: Vec<Val>) -> Result<Val, Trap> {
        self.calls += 1;
        let func = &self.m.funcs[f as usize];
        let mut env: Vec<Option<Val>> = vec![None; func.values.len()];
        let mut block = func.entry;
        let mut incoming = args;
        loop {
            let blk = &func.blocks[block as usize];
            for (p, a) in blk.params.iter().zip(incoming.drain(..)) {
                env[*p as usize] = Some(a);
            }
            for &v in &blk.insts {
                self.steps += 1;
                let ValueDef::Inst(inst) = &func.values[v as usize] else { unreachable!() };
                let val = self.exec(inst, &env)?;
                env[v as usize] = Some(val);
            }
            let get = |x: V| env[x as usize].clone().unwrap_or_else(|| panic!("v{x} used before it was defined in {}", func.name));
            match &blk.term {
                Term::Jump(t) => {
                    incoming = t.args.iter().map(|&a| get(a)).collect();
                    block = t.block;
                }
                Term::Branch(c, t, e) => {
                    let target = if get(*c).int() != 0 { t } else { e };
                    incoming = target.args.iter().map(|&a| get(a)).collect();
                    block = target.block;
                }
                Term::Return(v) => return Ok(get(*v)),
                Term::Panic(s) => return Err(Trap(self.m.strings[*s as usize].clone())),
                Term::Unreachable => panic!("reached an unreachable block in {}", func.name),
            }
        }
    }

    fn closure_for(&mut self, f: FuncId) -> Rc<Closure> {
        self.statics.entry(f).or_insert_with(|| Rc::new(Closure { func: f, captures: vec![] })).clone()
    }

    fn exec(&mut self, inst: &Inst, env: &[Option<Val>]) -> Result<Val, Trap> {
        let get = |x: &V| env[*x as usize].clone().expect("operand defined before use");
        Ok(match inst {
            Inst::Const(n) => Val::Int(*n),
            Inst::Str(s) => Val::Str(self.strings[*s as usize].clone()),
            Inst::Ctor0(c) => Val::Obj(self.ctor0[*c as usize].clone()),
            Inst::FuncRef(f) => Val::Clo(self.closure_for(*f)),
            Inst::Unary(UnOp::Neg, a) => Val::Int(wrap63(get(a).int().wrapping_neg())),
            Inst::Unary(UnOp::Not, a) => Val::Int(i64::from(get(a).int() == 0)),
            Inst::Binary(op, a, b) => {
                let (x, y) = (get(a), get(b));
                if matches!(op, BinOp::Eq | BinOp::Ne) {
                    let eq = match (&x, &y) {
                        (Val::Int(p), Val::Int(q)) => p == q,
                        _ => equal(&x, &y),
                    };
                    return Ok(Val::Int(i64::from(eq == (*op == BinOp::Eq))));
                }
                let (x, y) = (x.int(), y.int());
                Val::Int(match op {
                    BinOp::Add => wrap63(x.wrapping_add(y)),
                    BinOp::Sub => wrap63(x.wrapping_sub(y)),
                    BinOp::Mul => wrap63(x.wrapping_mul(y)),
                    BinOp::Div => {
                        if y == 0 {
                            return Err(Trap("division by zero".into()));
                        }
                        wrap63(x.wrapping_div(y))
                    }
                    BinOp::Mod => {
                        if y == 0 {
                            return Err(Trap("division by zero".into()));
                        }
                        wrap63(x.wrapping_rem(y))
                    }
                    BinOp::Lt => i64::from(x < y),
                    BinOp::Le => i64::from(x <= y),
                    BinOp::Gt => i64::from(x > y),
                    BinOp::Ge => i64::from(x >= y),
                    BinOp::Eq | BinOp::Ne => unreachable!(),
                })
            }
            Inst::Call(f, args) => {
                let vs = args.iter().map(get).collect();
                self.call(*f, vs)?
            }
            Inst::CallClosure(c, args) => {
                let cv = get(c);
                let Val::Clo(clo) = &cv else { panic!("calling a non-closure {cv:?}") };
                let mut vs: Vec<Val> = Vec::with_capacity(args.len() + 1);
                vs.push(cv.clone());
                vs.extend(args.iter().map(get));
                self.call(clo.func, vs)?
            }
            Inst::CallBuiltin(b, args) => {
                let vs: Vec<Val> = args.iter().map(get).collect();
                self.builtin(*b, &vs)?
            }
            Inst::Alloc { tag, fields } => Val::Obj(Rc::new(Obj { tag: *tag, fields: fields.iter().map(get).collect() })),
            Inst::MakeClosure { func, captures } => {
                Val::Clo(Rc::new(Closure { func: *func, captures: captures.iter().map(get).collect() }))
            }
            Inst::Field(a, i) => match get(a) {
                Val::Obj(o) => o.fields[*i as usize].clone(),
                Val::Clo(c) => c.captures[*i as usize - 1].clone(),
                other => panic!("field of a non-object {other:?}"),
            },
            Inst::Tag(a) => match get(a) {
                Val::Obj(o) => Val::Int(i64::from(o.tag)),
                other => panic!("tag of a non-object {other:?}"),
            },
        })
    }

    fn builtin(&mut self, b: Builtin, a: &[Val]) -> Result<Val, Trap> {
        let unit = Val::Int(0);
        Ok(match b {
            Builtin::Print => {
                let _ = self.out.write_all(a[0].str().as_bytes());
                unit
            }
            Builtin::Println => {
                let _ = self.out.write_all(a[0].str().as_bytes());
                let _ = self.out.write_all(b"\n");
                unit
            }
            Builtin::PrintInt => {
                let _ = writeln!(self.out, "{}", a[0].int());
                unit
            }
            Builtin::IntToString => Val::Str(Rc::from(a[0].int().to_string().as_str())),
            Builtin::StringLength => Val::Int(a[0].str().len() as i64),
            Builtin::StringGet => {
                let s = a[0].str().as_bytes();
                let i = a[1].int();
                if i < 0 || i as usize >= s.len() {
                    return Err(Trap(format!("string index {i} out of bounds (length {})", s.len())));
                }
                Val::Int(i64::from(s[i as usize]))
            }
            Builtin::Substring => {
                let s = a[0].str().as_bytes();
                let (start, len) = (a[1].int(), a[2].int());
                if start < 0 || len < 0 || start.checked_add(len).is_none_or(|e| e as usize > s.len()) {
                    return Err(Trap(format!("substring({start}, {len}) out of bounds (length {})", s.len())));
                }
                let bytes = &s[start as usize..(start + len) as usize];
                Val::Str(Rc::from(String::from_utf8_lossy(bytes).as_ref()))
            }
            Builtin::CharToString => {
                let n = a[0].int();
                if !(0..=255).contains(&n) {
                    return Err(Trap(format!("byte value {n} out of range 0..255")));
                }
                // Bytes above 127 are not valid UTF-8 on their own; the interpreter stores them as
                // Latin-1 characters, which is only visible if such strings are printed.
                Val::Str(Rc::from(char::from(n as u8).to_string().as_str()))
            }
            Builtin::StringToInt => {
                let s = a[0].str();
                match s.trim().parse::<i64>() {
                    Ok(n) if n == wrap63(n) => Val::Int(n),
                    _ => return Err(Trap(format!("not an integer: \"{s}\""))),
                }
            }
            Builtin::ArrayMake => {
                let n = a[0].int();
                if n < 0 {
                    return Err(Trap(format!("array_make: negative length {n}")));
                }
                Val::Arr(Rc::new(RefCell::new(vec![a[1].clone(); n as usize])))
            }
            Builtin::ArrayGet => {
                let Val::Arr(arr) = &a[0] else { unreachable!() };
                let arr = arr.borrow();
                let i = a[1].int();
                if i < 0 || i as usize >= arr.len() {
                    return Err(Trap(format!("array index {i} out of bounds (length {})", arr.len())));
                }
                arr[i as usize].clone()
            }
            Builtin::ArraySet => {
                let Val::Arr(arr) = &a[0] else { unreachable!() };
                let mut arr = arr.borrow_mut();
                let i = a[1].int();
                if i < 0 || i as usize >= arr.len() {
                    return Err(Trap(format!("array index {i} out of bounds (length {})", arr.len())));
                }
                arr[i as usize] = a[2].clone();
                unit
            }
            Builtin::ArrayLength => {
                let Val::Arr(arr) = &a[0] else { unreachable!() };
                Val::Int(arr.borrow().len() as i64)
            }
            Builtin::Panic => return Err(Trap(a[0].str().to_string())),
            Builtin::Assert => {
                if a[0].int() == 0 {
                    return Err(Trap("assertion failed".into()));
                }
                unit
            }
            Builtin::ClockMs => Val::Int(self.start.elapsed().as_millis() as i64),
            Builtin::GcCollect => unit,
            Builtin::GcCount => Val::Int(0),
            Builtin::Concat => {
                let mut s = String::with_capacity(a[0].str().len() + a[1].str().len());
                s.push_str(a[0].str());
                s.push_str(a[1].str());
                Val::Str(Rc::from(s.as_str()))
            }
            Builtin::Equal => Val::Int(i64::from(equal(&a[0], &a[1]))),
        })
    }
}

/// Runs a whole program, collecting its output. Runs on a thread with a large stack, since Quill
/// programs are recursive and so is the interpreter.
pub fn run_program(m: &Module) -> (String, Option<Trap>) {
    let m = m.clone();
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let mut out: Vec<u8> = Vec::new();
            let r = {
                let mut it = Interp::new(&m, &mut out);
                it.run_main()
            };
            (String::from_utf8_lossy(&out).into_owned(), r.err())
        })
        .expect("spawn interpreter thread")
        .join()
        .unwrap_or_else(|_| (String::new(), Some(Trap("interpreter crashed".into()))))
}

/// Shows a value the way the REPL prints it, using its type to name constructors.
pub fn show_value(v: &Val, ty: &crate::types::Ty, t: &crate::typeck::Typed) -> String {
    use crate::types::{Ty, subst_gen};
    let mut s = String::new();
    match (v, ty) {
        (Val::Int(n), Ty::Con(name, _)) if &**name == "Bool" => s.push_str(if *n != 0 { "true" } else { "false" }),
        (Val::Int(_), Ty::Tuple(es)) if es.is_empty() => s.push_str("()"),
        (Val::Int(n), _) => {
            let _ = write!(s, "{n}");
        }
        (Val::Str(x), _) => {
            let _ = write!(s, "{:?}", &**x);
        }
        (Val::Obj(o), Ty::Tuple(es)) => {
            let parts: Vec<String> = o.fields.iter().zip(es).map(|(f, t2)| show_value(f, t2, t)).collect();
            let _ = write!(s, "({})", parts.join(", "));
        }
        (Val::Obj(o), Ty::Con(name, args)) => {
            let ctor = t.adt_index.get(&**name).and_then(|&i| t.adts[i].ctors.get(o.tag as usize));
            match ctor {
                Some(c) => {
                    s.push_str(&c.name);
                    if !o.fields.is_empty() {
                        let parts: Vec<String> =
                            o.fields.iter().zip(&c.fields).map(|(f, ft)| show_value(f, &subst_gen(ft, args), t)).collect();
                        let _ = write!(s, "({})", parts.join(", "));
                    }
                }
                None => s.push_str("<value>"),
            }
        }
        (Val::Arr(a), Ty::Con(_, args)) => {
            let elem = args.first().cloned().unwrap_or_else(Ty::unit);
            let parts: Vec<String> = a.borrow().iter().take(100).map(|x| show_value(x, &elem, t)).collect();
            let _ = write!(s, "[{}{}]", parts.join(", "), if a.borrow().len() > 100 { ", ..." } else { "" });
        }
        (Val::Clo(_), _) => s.push_str("<function>"),
        _ => s.push_str("<value>"),
    }
    s
}
