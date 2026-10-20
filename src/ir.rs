//! The intermediate representation: functions as control-flow graphs of basic blocks in SSA form.
//!
//! Instead of phi nodes, blocks take parameters and jumps pass arguments (as in Cranelift and MLIR), which
//! keeps every value defined in exactly one place and makes SSA destruction a matter of parallel copies.
//!
//! Values are Quill values: integers are mathematical 63-bit integers, booleans are 0/1, unit is 0, and
//! everything else is a reference to an object. How they are represented in machine words is the
//! backend's business.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use crate::builtins::{self, Builtin};

pub type V = u32;
pub type BlockId = u32;
pub type FuncId = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Lt,
    Le,
    Gt,
    Ge,
    /// Word equality: for integers, booleans and unit (structural equality is a builtin call).
    Eq,
    Ne,
}

impl BinOp {
    pub fn name(self) -> &'static str {
        match self {
            BinOp::Add => "add",
            BinOp::Sub => "sub",
            BinOp::Mul => "mul",
            BinOp::Div => "div",
            BinOp::Mod => "mod",
            BinOp::Lt => "lt",
            BinOp::Le => "le",
            BinOp::Gt => "gt",
            BinOp::Ge => "ge",
            BinOp::Eq => "eq",
            BinOp::Ne => "ne",
        }
    }

    pub fn is_compare(self) -> bool {
        matches!(self, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne)
    }

    /// Can this operation fail (division by zero)?
    pub fn can_trap(self) -> bool {
        matches!(self, BinOp::Div | BinOp::Mod)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Inst {
    /// An integer, boolean (0/1) or unit (0).
    Const(i64),
    /// A string literal (index into the module's string table).
    Str(u32),
    /// A constructor without fields (index into the module's nullary-constructor table).
    Ctor0(u32),
    /// The closure object of a function that takes an environment (a static object, no captures).
    FuncRef(FuncId),
    Unary(UnOp, V),
    Binary(BinOp, V, V),
    /// A direct call.
    Call(FuncId, Vec<V>),
    /// A call through a closure: the closure itself is passed as the first argument.
    CallClosure(V, Vec<V>),
    CallBuiltin(Builtin, Vec<V>),
    /// A new object with a constructor tag and fields (also tuples, with tag 0).
    Alloc {
        tag: u32,
        fields: Vec<V>,
    },
    /// A new closure for a function whose first parameter is the environment.
    MakeClosure {
        func: FuncId,
        captures: Vec<V>,
    },
    /// Field `i` of an object (constructor fields from 0, tuples from 0, closure captures from 1).
    Field(V, u32),
    /// The constructor tag of an ADT value.
    Tag(V),
}

impl Inst {
    pub fn operands(&self) -> Vec<V> {
        match self {
            Inst::Const(_) | Inst::Str(_) | Inst::Ctor0(_) | Inst::FuncRef(_) => vec![],
            Inst::Unary(_, a) | Inst::Field(a, _) | Inst::Tag(a) => vec![*a],
            Inst::Binary(_, a, b) => vec![*a, *b],
            Inst::Call(_, args) | Inst::CallBuiltin(_, args) => args.clone(),
            Inst::CallClosure(f, args) => std::iter::once(*f).chain(args.iter().copied()).collect(),
            Inst::Alloc { fields, .. } => fields.clone(),
            Inst::MakeClosure { captures, .. } => captures.clone(),
        }
    }

    pub fn map_operands(&mut self, f: &mut impl FnMut(V) -> V) {
        match self {
            Inst::Const(_) | Inst::Str(_) | Inst::Ctor0(_) | Inst::FuncRef(_) => {}
            Inst::Unary(_, a) | Inst::Field(a, _) | Inst::Tag(a) => *a = f(*a),
            Inst::Binary(_, a, b) => {
                *a = f(*a);
                *b = f(*b);
            }
            Inst::Call(_, args) | Inst::CallBuiltin(_, args) => args.iter_mut().for_each(|a| *a = f(*a)),
            Inst::CallClosure(c, args) => {
                *c = f(*c);
                args.iter_mut().for_each(|a| *a = f(*a));
            }
            Inst::Alloc { fields, .. } => fields.iter_mut().for_each(|a| *a = f(*a)),
            Inst::MakeClosure { captures, .. } => captures.iter_mut().for_each(|a| *a = f(*a)),
        }
    }

    /// Without side effects: may be removed if unused, and computed once if repeated.
    pub fn is_pure(&self) -> bool {
        match self {
            Inst::Const(_) | Inst::Str(_) | Inst::Ctor0(_) | Inst::FuncRef(_) | Inst::Unary(..) | Inst::Field(..) | Inst::Tag(_) => true,
            Inst::Binary(op, _, _) => !op.can_trap(),
            Inst::CallBuiltin(b, _) => builtins::info(*b).pure,
            Inst::Call(..) | Inst::CallClosure(..) => false,
            // Allocation has no visible effect if the object is never used.
            Inst::Alloc { .. } | Inst::MakeClosure { .. } => true,
        }
    }

    /// May be computed once for identical operands (allocations may not: each one is a new object).
    pub fn is_cse_candidate(&self) -> bool {
        self.is_pure() && !matches!(self, Inst::Alloc { .. } | Inst::MakeClosure { .. } | Inst::CallBuiltin(..))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub block: BlockId,
    pub args: Vec<V>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Jump(Target),
    Branch(V, Target, Target),
    Return(V),
    /// Stops the program with a message (a string-table index).
    Panic(u32),
    /// A block that is never reached (before cleanup).
    Unreachable,
}

impl Term {
    pub fn targets(&self) -> Vec<&Target> {
        match self {
            Term::Jump(t) => vec![t],
            Term::Branch(_, a, b) => vec![a, b],
            _ => vec![],
        }
    }

    pub fn targets_mut(&mut self) -> Vec<&mut Target> {
        match self {
            Term::Jump(t) => vec![t],
            Term::Branch(_, a, b) => vec![a, b],
            _ => vec![],
        }
    }

    pub fn successors(&self) -> Vec<BlockId> {
        self.targets().iter().map(|t| t.block).collect()
    }

    pub fn operands(&self) -> Vec<V> {
        let mut out = Vec::new();
        match self {
            Term::Jump(t) => out.extend(&t.args),
            Term::Branch(c, a, b) => {
                out.push(*c);
                out.extend(&a.args);
                out.extend(&b.args);
            }
            Term::Return(v) => out.push(*v),
            _ => {}
        }
        out
    }

    pub fn map_operands(&mut self, f: &mut impl FnMut(V) -> V) {
        match self {
            Term::Jump(t) => t.args.iter_mut().for_each(|a| *a = f(*a)),
            Term::Branch(c, a, b) => {
                *c = f(*c);
                a.args.iter_mut().for_each(|x| *x = f(*x));
                b.args.iter_mut().for_each(|x| *x = f(*x));
            }
            Term::Return(v) => *v = f(*v),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub struct Block {
    pub params: Vec<V>,
    pub insts: Vec<V>,
    pub term: Term,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ValueDef {
    Inst(Inst),
    Param { block: BlockId, index: u32 },
}

#[derive(Clone, Debug)]
pub struct Function {
    pub name: String,
    pub entry: BlockId,
    pub blocks: Vec<Block>,
    pub values: Vec<ValueDef>,
    /// The first parameter is a closure environment.
    pub takes_env: bool,
    /// Set by inlining when a block is no longer used (blocks are never removed, only emptied).
    pub dead_blocks: HashSet<BlockId>,
}

impl Function {
    pub fn new(name: impl Into<String>, takes_env: bool) -> Function {
        let mut f = Function { name: name.into(), entry: 0, blocks: vec![], values: vec![], takes_env, dead_blocks: HashSet::new() };
        f.entry = f.new_block();
        f
    }

    pub fn new_block(&mut self) -> BlockId {
        self.blocks.push(Block { params: vec![], insts: vec![], term: Term::Unreachable });
        self.blocks.len() as BlockId - 1
    }

    pub fn add_param(&mut self, block: BlockId) -> V {
        let index = self.blocks[block as usize].params.len() as u32;
        self.values.push(ValueDef::Param { block, index });
        let v = self.values.len() as V - 1;
        self.blocks[block as usize].params.push(v);
        v
    }

    /// Creates an instruction value without placing it in a block.
    pub fn make(&mut self, inst: Inst) -> V {
        self.values.push(ValueDef::Inst(inst));
        self.values.len() as V - 1
    }

    pub fn push(&mut self, block: BlockId, inst: Inst) -> V {
        let v = self.make(inst);
        self.blocks[block as usize].insts.push(v);
        v
    }

    pub fn inst(&self, v: V) -> Option<&Inst> {
        match &self.values[v as usize] {
            ValueDef::Inst(i) => Some(i),
            ValueDef::Param { .. } => None,
        }
    }

    pub fn inst_mut(&mut self, v: V) -> Option<&mut Inst> {
        match &mut self.values[v as usize] {
            ValueDef::Inst(i) => Some(i),
            ValueDef::Param { .. } => None,
        }
    }

    pub fn params(&self) -> &[V] {
        &self.blocks[self.entry as usize].params
    }

    /// Blocks reachable from the entry, in reverse post-order.
    pub fn rpo(&self) -> Vec<BlockId> {
        let mut seen = vec![false; self.blocks.len()];
        let mut order = Vec::new();
        let mut stack = vec![(self.entry, 0usize)];
        seen[self.entry as usize] = true;
        while let Some((b, i)) = stack.pop() {
            let succs = self.blocks[b as usize].term.successors();
            if i < succs.len() {
                stack.push((b, i + 1));
                let s = succs[i];
                if !seen[s as usize] {
                    seen[s as usize] = true;
                    stack.push((s, 0));
                }
            } else {
                order.push(b);
            }
        }
        order.reverse();
        order
    }

    pub fn predecessors(&self) -> HashMap<BlockId, Vec<BlockId>> {
        let mut preds: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for b in self.rpo() {
            for s in self.blocks[b as usize].term.successors() {
                preds.entry(s).or_default().push(b);
            }
        }
        preds
    }

    /// Replaces every use of a value according to `map` (following chains).
    pub fn replace_uses(&mut self, map: &HashMap<V, V>) {
        if map.is_empty() {
            return;
        }
        let resolve = |mut v: V| {
            let mut steps = 0;
            while let Some(&n) = map.get(&v) {
                if n == v || steps > 10_000 {
                    break;
                }
                v = n;
                steps += 1;
            }
            v
        };
        for b in 0..self.blocks.len() {
            let insts = self.blocks[b].insts.clone();
            for v in insts {
                if let ValueDef::Inst(i) = &mut self.values[v as usize] {
                    i.map_operands(&mut |x| resolve(x));
                }
            }
            self.blocks[b].term.map_operands(&mut |x| resolve(x));
        }
    }

    /// Number of instructions (a size estimate for inlining).
    pub fn size(&self) -> usize {
        self.rpo().iter().map(|&b| self.blocks[b as usize].insts.len() + 1).sum()
    }
}

/// A nullary constructor, which exists once as a static object.
#[derive(Clone, Debug)]
pub struct Ctor0Info {
    pub adt: String,
    pub name: String,
    pub tag: u32,
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub funcs: Vec<Function>,
    pub strings: Vec<String>,
    pub ctor0: Vec<Ctor0Info>,
    pub main: FuncId,
}

impl Module {
    pub fn intern_string(&mut self, s: &str) -> u32 {
        if let Some(i) = self.strings.iter().position(|x| x == s) {
            return i as u32;
        }
        self.strings.push(s.to_string());
        self.strings.len() as u32 - 1
    }
}

// ----------------------------------------------------------------------
// Printing
// ----------------------------------------------------------------------

pub fn print_module(m: &Module) -> String {
    let mut out = String::new();
    for (i, f) in m.funcs.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&print_function(m, f));
    }
    out
}

pub fn print_function(m: &Module, f: &Function) -> String {
    let mut out = String::new();
    let params: Vec<String> = f.params().iter().map(|p| format!("v{p}")).collect();
    let _ = writeln!(out, "fn {}({}){} {{", f.name, params.join(", "), if f.takes_env { " env" } else { "" });
    for b in f.rpo() {
        let blk = &f.blocks[b as usize];
        let ps: Vec<String> = blk.params.iter().map(|p| format!("v{p}")).collect();
        let _ = writeln!(out, "b{b}({}):", ps.join(", "));
        for &v in &blk.insts {
            let _ = writeln!(out, "  v{v} = {}", print_inst(m, f.inst(v).expect("block holds instructions")));
        }
        let _ = writeln!(out, "  {}", print_term(m, &blk.term));
    }
    out.push_str("}\n");
    out
}

fn vs(args: &[V]) -> String {
    args.iter().map(|a| format!("v{a}")).collect::<Vec<_>>().join(", ")
}

fn print_inst(m: &Module, i: &Inst) -> String {
    let fname = |id: FuncId| m.funcs.get(id as usize).map_or_else(|| format!("f{id}"), |f| f.name.clone());
    match i {
        Inst::Const(n) => format!("const {n}"),
        Inst::Str(s) => format!("str {:?}", m.strings.get(*s as usize).map_or("?", |x| x.as_str())),
        Inst::Ctor0(c) => format!("ctor {}", m.ctor0.get(*c as usize).map_or("?", |x| x.name.as_str())),
        Inst::FuncRef(f) => format!("funcref {}", fname(*f)),
        Inst::Unary(op, a) => format!("{} v{a}", if *op == UnOp::Neg { "neg" } else { "not" }),
        Inst::Binary(op, a, b) => format!("{} v{a}, v{b}", op.name()),
        Inst::Call(f, args) => format!("call {}({})", fname(*f), vs(args)),
        Inst::CallClosure(c, args) => format!("call_closure v{c}({})", vs(args)),
        Inst::CallBuiltin(b, args) => format!("call_builtin {}({})", builtins::info(*b).symbol, vs(args)),
        Inst::Alloc { tag, fields } => format!("alloc tag {tag} [{}]", vs(fields)),
        Inst::MakeClosure { func, captures } => format!("closure {} [{}]", fname(*func), vs(captures)),
        Inst::Field(a, i) => format!("field v{a}.{i}"),
        Inst::Tag(a) => format!("tag v{a}"),
    }
}

fn print_target(t: &Target) -> String {
    if t.args.is_empty() { format!("b{}", t.block) } else { format!("b{}({})", t.block, vs(&t.args)) }
}

fn print_term(m: &Module, t: &Term) -> String {
    match t {
        Term::Jump(t) => format!("jump {}", print_target(t)),
        Term::Branch(c, a, b) => format!("br v{c}, {}, {}", print_target(a), print_target(b)),
        Term::Return(v) => format!("ret v{v}"),
        Term::Panic(s) => format!("panic {:?}", m.strings.get(*s as usize).map_or("?", |x| x.as_str())),
        Term::Unreachable => "unreachable".to_string(),
    }
}

// ----------------------------------------------------------------------
// Verification
// ----------------------------------------------------------------------

/// Checks the structural invariants every pass must preserve: values are defined once, every use is
/// dominated by its definition, jump arguments match block parameters, and every reachable block ends in a
/// real terminator. Run after every pass in tests.
pub fn verify(m: &Module) -> Result<(), String> {
    for f in &m.funcs {
        verify_function(m, f).map_err(|e| format!("in fn {}: {e}\n{}", f.name, print_function(m, f)))?;
    }
    Ok(())
}

pub fn verify_function(m: &Module, f: &Function) -> Result<(), String> {
    let order = f.rpo();
    let dom = crate::dom::DomTree::compute(f);
    let mut def_block: HashMap<V, (BlockId, usize)> = HashMap::new();
    for &b in &order {
        let blk = &f.blocks[b as usize];
        for (i, &p) in blk.params.iter().enumerate() {
            if f.values[p as usize] != (ValueDef::Param { block: b, index: i as u32 }) {
                return Err(format!("v{p} is listed as a parameter of b{b} but is not one"));
            }
            if def_block.insert(p, (b, 0)).is_some() {
                return Err(format!("v{p} defined twice"));
            }
        }
        for (i, &v) in blk.insts.iter().enumerate() {
            if f.inst(v).is_none() {
                return Err(format!("b{b} lists v{v}, which is not an instruction"));
            }
            if def_block.insert(v, (b, i + 1)).is_some() {
                return Err(format!("v{v} defined twice"));
            }
        }
    }
    let check_use = |u: V, at_block: BlockId, at_pos: usize| -> Result<(), String> {
        let Some(&(db, dpos)) = def_block.get(&u) else {
            return Err(format!("v{u} is used in b{at_block} but never defined"));
        };
        if db == at_block {
            if dpos > at_pos || (dpos == at_pos && at_pos != 0) {
                return Err(format!("v{u} is used in b{at_block} before it is defined"));
            }
        } else if !dom.dominates(db, at_block) {
            return Err(format!("v{u} (defined in b{db}) does not dominate its use in b{at_block}"));
        }
        Ok(())
    };
    for &b in &order {
        let blk = &f.blocks[b as usize];
        for (i, &v) in blk.insts.iter().enumerate() {
            for u in f.inst(v).unwrap().operands() {
                check_use(u, b, i + 1)?;
            }
            if let Some(Inst::Call(callee, args)) = f.inst(v) {
                let g = m.funcs.get(*callee as usize).ok_or("call to an unknown function")?;
                if g.params().len() != args.len() {
                    return Err(format!("call to {} with {} arguments, expected {}", g.name, args.len(), g.params().len()));
                }
            }
        }
        let end = blk.insts.len() + 1;
        for u in blk.term.operands() {
            check_use(u, b, end)?;
        }
        if matches!(blk.term, Term::Unreachable) {
            return Err(format!("reachable block b{b} has no terminator"));
        }
        for t in blk.term.targets() {
            let want = f.blocks[t.block as usize].params.len();
            if t.args.len() != want {
                return Err(format!("jump from b{b} to b{} passes {} arguments for {} parameters", t.block, t.args.len(), want));
            }
        }
    }
    Ok(())
}
