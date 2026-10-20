//! Assembly emission: applies the register allocation, lays out the stack frame, expands the
//! pseudo-instructions (calls, returns, parallel moves) and legalizes operand combinations x86 does not
//! have (memory-to-memory moves, spilled base registers, 64-bit immediates), then prints GNU assembler
//! Intel syntax.
//!
//! Frame layout (rbp-based):
//!
//! ```text
//!   [rbp + 16 + shadow + 8i]   incoming stack argument i
//!   [rbp + 8]                  return address
//!   [rbp]                      saved rbp
//!   [rbp - 8 .. ]              saved callee-saved registers
//!   ...                        spill slots
//!   [rsp + shadow + 8i]        outgoing stack argument i   ([rsp, rsp+32) is the Win64 shadow space)
//! ```

use std::fmt::Write as _;

use super::regalloc::{Allocation, Assign};
use super::x86::*;

/// An operand after register allocation.
#[derive(Clone, Debug, PartialEq)]
enum X {
    R(Reg),
    Imm(i64),
    /// `[base + disp]` with a register base.
    M(Reg, i32),
    /// `[base + disp]` where the base value lives in a spill slot.
    SlotBase(u32, i32),
    Slot(u32),
    Incoming(u32),
    Outgoing(u32),
}

impl X {
    fn is_mem(&self) -> bool {
        !matches!(self, X::R(_) | X::Imm(_))
    }

    fn mentions(&self, r: Reg) -> bool {
        matches!(self, X::R(x) | X::M(x, _) if *x == r)
    }
}

struct Frame {
    saved: Vec<Reg>,
    shadow: i32,
}

impl Frame {
    fn slot_offset(&self, s: u32) -> i32 {
        -(8 * (self.saved.len() as i32 + 1 + s as i32))
    }
}

pub struct Emitter<'a> {
    out: &'a mut String,
    abi: Abi,
    frame: Frame,
    sym: String,
}

pub fn emit_function(out: &mut String, f: &MFunction, alloc: &Allocation, abi: Abi, global_alias: Option<&str>) {
    let shadow = abi.shadow_space();
    let frame = Frame { saved: alloc.callee_saved_used.clone(), shadow };
    let outgoing = shadow + 8 * f.max_stack_args as i32;
    let mut sub = 8 * alloc.slots as i32 + outgoing;
    // After `push rbp` the stack is 16-byte aligned; keep it so at every call.
    if (8 * frame.saved.len() as i32 + sub) % 16 != 0 {
        sub += 8;
    }
    let mut e = Emitter { out, abi, frame, sym: f.symbol.clone() };
    let _ = writeln!(e.out, "\n    .p2align 4");
    let _ = writeln!(e.out, "    .globl {}", f.symbol);
    if let Some(alias) = global_alias {
        let _ = writeln!(e.out, "    .globl {alias}");
        let _ = writeln!(e.out, "{alias}:");
    }
    let _ = writeln!(e.out, "{}:  # {}", f.symbol, f.name);
    e.line("push rbp");
    e.line("mov rbp, rsp");
    for r in &e.frame.saved.clone() {
        e.line(&format!("push {r}"));
    }
    if sub > 0 {
        e.line(&format!("sub rsp, {sub}"));
    }
    let map = |o: &Op| -> X {
        match o {
            Op::L(Loc::R(r)) => X::R(*r),
            Op::L(Loc::V(v)) => match alloc.assign[*v as usize] {
                Some(Assign::Reg(r)) => X::R(r),
                Some(Assign::Slot(s)) => X::Slot(s),
                // A value that is never live (defined and unused) can go anywhere harmless.
                None => X::R(Reg::R11),
            },
            Op::Imm(k) => X::Imm(*k),
            Op::Mem(Loc::R(r), d) => X::M(*r, *d),
            Op::Mem(Loc::V(v), d) => match alloc.assign[*v as usize] {
                Some(Assign::Reg(r)) => X::M(r, *d),
                Some(Assign::Slot(s)) => X::SlotBase(s, *d),
                None => X::M(Reg::R11, *d),
            },
            Op::Slot(s) => X::Slot(*s),
            Op::Incoming(i) => X::Incoming(*i),
            Op::Outgoing(i) => X::Outgoing(*i),
        }
    };
    let maploc = |l: &Loc| -> X {
        match l {
            Loc::R(r) => X::R(*r),
            Loc::V(v) => match alloc.assign[*v as usize] {
                Some(Assign::Reg(r)) => X::R(r),
                Some(Assign::Slot(s)) => X::Slot(s),
                None => X::R(Reg::R11),
            },
        }
    };
    for (bi, b) in f.blocks.iter().enumerate() {
        let _ = writeln!(e.out, ".L{}_{}:", e.sym, bi);
        for (ii, inst) in b.insts.iter().enumerate() {
            // A jump to the next block is a fall-through.
            if let MI::Jmp(t) = inst
                && *t as usize == bi + 1
                && ii + 1 == b.insts.len()
            {
                continue;
            }
            e.inst(inst, &map, &maploc);
        }
    }
    let _ = writeln!(e.out, ".L{}_ret:", e.sym);
    let saved = e.frame.saved.clone();
    if !saved.is_empty() {
        e.line(&format!("lea rsp, [rbp - {}]", 8 * saved.len()));
        for r in saved.iter().rev() {
            e.line(&format!("pop {r}"));
        }
    } else {
        e.line("mov rsp, rbp");
    }
    e.line("pop rbp");
    e.line("ret");
}

impl<'a> Emitter<'a> {
    fn line(&mut self, s: &str) {
        let _ = writeln!(self.out, "    {s}");
    }

    fn fmt(&self, x: &X) -> String {
        match x {
            X::R(r) => r.name().to_string(),
            X::Imm(k) => k.to_string(),
            X::M(r, d) => mem(*r, *d),
            X::Slot(s) => mem(Reg::Rbp, self.frame.slot_offset(*s)),
            X::Incoming(i) => mem(Reg::Rbp, 16 + self.frame.shadow + 8 * *i as i32),
            X::Outgoing(i) => mem(Reg::Rsp, self.frame.shadow + 8 * *i as i32),
            X::SlotBase(..) => unreachable!("spilled bases are loaded first"),
        }
    }

    /// Temporary registers not mentioned by the given operands (r11 first: instruction selection never
    /// uses it).
    fn temps(&self, xs: &[&X]) -> Vec<Reg> {
        [Reg::R11, Reg::R10, Reg::Rax].into_iter().filter(|r| !xs.iter().any(|x| x.mentions(*r))).collect()
    }

    /// Makes a memory operand addressable: a spilled base is loaded into a temporary taken from `temps`.
    fn addressable(&mut self, x: X, temps: &mut Vec<Reg>) -> X {
        match x {
            X::SlotBase(s, d) => {
                let tmp = temps.remove(0);
                let slot = self.fmt(&X::Slot(s));
                self.line(&format!("mov {tmp}, {slot}"));
                X::M(tmp, d)
            }
            other => other,
        }
    }

    fn mov(&mut self, d: &X, s: &X) {
        if d == s {
            return;
        }
        let mut temps = self.temps(&[d, s]);
        let d = self.addressable(d.clone(), &mut temps);
        let s = self.addressable(s.clone(), &mut temps);
        let fits = |k: i64| i32::try_from(k).is_ok();
        match (&d, &s) {
            (X::R(r), X::Imm(k)) if !fits(*k) => self.line(&format!("movabs {r}, {k}")),
            (_, X::Imm(k)) if d.is_mem() && !fits(*k) => {
                let t = temps[0];
                self.line(&format!("movabs {t}, {k}"));
                let dd = self.fmt(&d);
                self.line(&format!("mov {dd}, {t}"));
            }
            _ if d.is_mem() && s.is_mem() => {
                let t = temps[0];
                let ss = self.fmt(&s);
                self.line(&format!("mov {t}, {ss}"));
                let dd = self.fmt(&d);
                self.line(&format!("mov {dd}, {t}"));
            }
            _ => {
                let (dd, ss) = (self.fmt(&d), self.fmt(&s));
                let size = if d.is_mem() && matches!(s, X::Imm(_)) { "qword ptr " } else { "" };
                self.line(&format!("mov {size}{dd}, {ss}"));
            }
        }
    }

    fn inst(&mut self, inst: &MI, map: &dyn Fn(&Op) -> X, maploc: &dyn Fn(&Loc) -> X) {
        match inst {
            MI::Mov(d, s) => {
                let (d, s) = (map(d), map(s));
                self.mov(&d, &s);
            }
            MI::LeaSym(d, sym) => {
                let d = map(d);
                match &d {
                    X::R(r) => self.line(&format!("lea {r}, [rip + {sym}]")),
                    _ => {
                        let t = self.temps(&[&d])[0];
                        self.line(&format!("lea {t}, [rip + {sym}]"));
                        self.mov(&d, &X::R(t));
                    }
                }
            }
            MI::Lea { dst, base, index, scale, disp } => {
                let d = map(dst);
                let b = maploc(base);
                let ix = index.as_ref().map(maploc);
                let mut temps = self.temps(&[&d, &b, ix.as_ref().unwrap_or(&X::Imm(0))]);
                let breg = match b {
                    X::R(r) => r,
                    other => {
                        let t = temps.remove(0);
                        self.mov(&X::R(t), &other);
                        t
                    }
                };
                let ireg = match ix {
                    None => None,
                    Some(X::R(r)) => Some(r),
                    Some(other) => {
                        let t = temps.remove(0);
                        self.mov(&X::R(t), &other);
                        Some(t)
                    }
                };
                let addr = match ireg {
                    None => format!("[{breg}{}]", disp_str(*disp)),
                    Some(i) => format!("[{breg} + {i}*{scale}{}]", disp_str(*disp)),
                };
                match &d {
                    X::R(r) => self.line(&format!("lea {r}, {addr}")),
                    _ => {
                        let t = temps[0];
                        self.line(&format!("lea {t}, {addr}"));
                        self.mov(&d, &X::R(t));
                    }
                }
            }
            MI::Alu(op, d, s) => {
                let (d, s) = (map(d), map(s));
                self.alu(*op, d, s);
            }
            MI::IMul3(d, s, k) => {
                let (d, s) = (map(d), map(s));
                let mut temps = self.temps(&[&d, &s]);
                let s = self.addressable(s, &mut temps);
                let dr = match &d {
                    X::R(r) => *r,
                    _ => temps[0],
                };
                let ss = self.fmt(&s);
                self.line(&format!("imul {dr}, {ss}, {k}"));
                if !matches!(d, X::R(_)) {
                    self.mov(&d, &X::R(dr));
                }
            }
            MI::Shift(op, d, n) => {
                let d = map(d);
                let mut temps = self.temps(&[&d]);
                let d = self.addressable(d, &mut temps);
                let name = match op {
                    Shift::Sar => "sar",
                    Shift::Shr => "shr",
                    Shift::Shl => "shl",
                };
                let dd = self.fmt(&d);
                let size = if d.is_mem() { "qword ptr " } else { "" };
                self.line(&format!("{name} {size}{dd}, {n}"));
            }
            MI::Neg(d) => {
                let d = map(d);
                let mut temps = self.temps(&[&d]);
                let d = self.addressable(d, &mut temps);
                let dd = self.fmt(&d);
                let size = if d.is_mem() { "qword ptr " } else { "" };
                self.line(&format!("neg {size}{dd}"));
            }
            MI::SetCC(c) => {
                self.line(&format!("set{} al", c.name()));
                self.line("movzx eax, al");
            }
            MI::Cqo => self.line("cqo"),
            MI::IDiv(s) => {
                let s = map(s);
                let mut temps = self.temps(&[&s]);
                let s = self.addressable(s, &mut temps);
                let ss = self.fmt(&s);
                let size = if s.is_mem() { "qword ptr " } else { "" };
                self.line(&format!("idiv {size}{ss}"));
            }
            MI::Jmp(b) => self.line(&format!("jmp .L{}_{}", self.sym, b)),
            MI::Jcc(c, b) => self.line(&format!("j{} .L{}_{}", c.name(), self.sym, b)),
            MI::Ret(v) => {
                let v = map(v);
                self.mov(&X::R(Reg::Rax), &v);
                self.line(&format!("jmp .L{}_ret", self.sym));
            }
            MI::ParMove(moves) => {
                let moves: Vec<(X, X)> = moves.iter().map(|(d, s)| (map(d), map(s))).collect();
                self.parallel_move(moves);
            }
            MI::Call { target, args, ret } => {
                if let CallTarget::Closure(c) = target {
                    // The code pointer, loaded before the arguments are shuffled into place.
                    let c = map(c);
                    self.mov(&X::R(Reg::Rax), &c);
                    self.line("mov rax, [rax + 8]");
                }
                let regs = self.abi.arg_regs();
                let moves: Vec<(X, X)> = args
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (if i < regs.len() { X::R(regs[i]) } else { X::Outgoing((i - regs.len()) as u32) }, map(a)))
                    .collect();
                self.parallel_move(moves);
                match target {
                    CallTarget::Sym(s) => self.line(&format!("call {s}")),
                    CallTarget::Closure(_) => self.line("call rax"),
                }
                if let Some(r) = ret {
                    let r = map(r);
                    self.mov(&r, &X::R(Reg::Rax));
                }
            }
            MI::CallNoReturn { sym, args } => {
                let regs = self.abi.arg_regs();
                let moves: Vec<(X, X)> = args.iter().enumerate().map(|(i, a)| (X::R(regs[i]), map(a))).collect();
                self.parallel_move(moves);
                self.line(&format!("call {sym}"));
            }
            MI::IMulWide(s) => {
                let s = map(s);
                let mut temps = self.temps(&[&s, &X::R(Reg::Rax)]);
                let s = self.addressable(s, &mut temps);
                let ss = self.fmt(&s);
                let size = if s.is_mem() { "qword ptr " } else { "" };
                self.line(&format!("imul {size}{ss}"));
            }
            MI::Load { dst, base, index, scale, disp, byte } => {
                let d = map(dst);
                let (b, ix) = (maploc(base), maploc(index));
                let mut temps = self.temps(&[&d, &b, &ix]);
                let addr = self.indexed(b, ix, *scale, *disp, &mut temps);
                let dr = match &d {
                    X::R(r) => *r,
                    _ => temps[0],
                };
                if *byte {
                    self.line(&format!("movzx {}, byte ptr {addr}", dr.name32()));
                } else {
                    self.line(&format!("mov {dr}, {addr}"));
                }
                if !matches!(d, X::R(_)) {
                    self.mov(&d, &X::R(dr));
                }
            }
            MI::Store { base, index, scale, disp, src } => {
                let s = map(src);
                let (b, ix) = (maploc(base), maploc(index));
                let mut temps = self.temps(&[&s, &b, &ix]);
                let addr = self.indexed(b, ix, *scale, *disp, &mut temps);
                let fits = |k: i64| i32::try_from(k).is_ok();
                match s {
                    X::R(r) => self.line(&format!("mov {addr}, {r}")),
                    X::Imm(k) if fits(k) => self.line(&format!("mov qword ptr {addr}, {k}")),
                    other => {
                        let t = temps[0];
                        self.mov(&X::R(t), &other);
                        self.line(&format!("mov {addr}, {t}"));
                    }
                }
            }
        }
    }

    /// The address `[base + index*scale + disp]`, loading spilled registers into temporaries.
    fn indexed(&mut self, b: X, ix: X, scale: u8, disp: i32, temps: &mut Vec<Reg>) -> String {
        let mut reg = |e: &mut Self, x: X| match x {
            X::R(r) => r,
            other => {
                let t = temps.remove(0);
                e.mov(&X::R(t), &other);
                t
            }
        };
        let br = reg(self, b);
        let ir = reg(self, ix);
        format!("[{br} + {ir}*{scale}{}]", disp_str(disp))
    }

    fn alu(&mut self, op: Alu, d: X, s: X) {
        let mut temps = self.temps(&[&d, &s]);
        let d0 = d.clone();
        let mut d = self.addressable(d, &mut temps);
        let mut s = self.addressable(s, &mut temps);
        let fits = |k: i64| i32::try_from(k).is_ok();
        let mut writeback: Option<(X, Reg)> = None;
        // An immediate first operand (only cmp/test can have one): load it.
        if matches!(d, X::Imm(_)) {
            let t = temps.remove(0);
            self.mov(&X::R(t), &d);
            d = X::R(t);
        }
        if let X::Imm(k) = s
            && !fits(k)
        {
            let t = temps.remove(0);
            self.line(&format!("movabs {t}, {k}"));
            s = X::R(t);
        }
        if d.is_mem() && (s.is_mem() || op == Alu::IMul) {
            if op == Alu::IMul {
                // imul needs a register destination.
                let t = temps.remove(0);
                self.mov(&X::R(t), &d);
                writeback = Some((d0, t));
                d = X::R(t);
            } else {
                let t = temps.remove(0);
                self.mov(&X::R(t), &s);
                s = X::R(t);
            }
        }
        let size = if d.is_mem() && matches!(s, X::Imm(_)) { "qword ptr " } else { "" };
        let (dd, ss) = (self.fmt(&d), self.fmt(&s));
        self.line(&format!("{} {size}{dd}, {ss}", op.name()));
        if let Some((orig, t)) = writeback {
            self.mov(&orig, &X::R(t));
        }
    }

    /// Emits a set of moves that conceptually happen at once (block arguments, call arguments).
    fn parallel_move(&mut self, moves: Vec<(X, X)>) {
        let mut pending: Vec<(X, X)> = moves.into_iter().filter(|(d, s)| d != s).collect();
        while !pending.is_empty() {
            // A move whose destination no other pending move still reads can go now.
            let ready = pending.iter().position(|(d, _)| !pending.iter().any(|(_, s)| s == d));
            match ready {
                Some(i) => {
                    let (d, s) = pending.remove(i);
                    self.mov_via(&d, &s, Reg::R10);
                }
                None => {
                    // A cycle: save one destination's current value in r11 and redirect its readers.
                    let d = pending[0].0.clone();
                    self.mov_via(&X::R(Reg::R11), &d, Reg::R10);
                    for (_, s) in pending.iter_mut() {
                        if *s == d {
                            *s = X::R(Reg::R11);
                        }
                    }
                }
            }
        }
    }

    /// A move that uses `tmp` (and only it) when it needs a scratch register.
    fn mov_via(&mut self, d: &X, s: &X, tmp: Reg) {
        if d.is_mem() && (s.is_mem() || matches!(s, X::Imm(k) if i32::try_from(*k).is_err())) {
            self.mov(&X::R(tmp), s);
            self.mov(d, &X::R(tmp));
        } else {
            self.mov(d, s);
        }
    }
}

fn mem(base: Reg, disp: i32) -> String {
    format!("qword ptr [{base}{}]", disp_str(disp))
}

fn disp_str(d: i32) -> String {
    match d.cmp(&0) {
        std::cmp::Ordering::Equal => String::new(),
        std::cmp::Ordering::Greater => format!(" + {d}"),
        std::cmp::Ordering::Less => format!(" - {}", -(d as i64)),
    }
}
