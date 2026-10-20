//! Instruction selection: IR to machine IR with virtual registers.
//!
//! Value representation: every Quill value is one 64-bit word. An integer n is stored as `2n + 1` (so its
//! lowest bit is 1); booleans and unit are the integers 0/1 and 0; everything else is a pointer to an
//! 8-byte-aligned object (lowest bit 0). The garbage collector tells pointers from integers by that bit,
//! without any type information. Arithmetic works on the tagged form directly:
//!
//! ```text
//! a + b  ->  a + b - 1            a - b  ->  a - b + 1
//! a * b  ->  (a >> 1) * (b - 1) + 1
//! a < b  ->  compare a, b directly (tagging preserves order), result 2*flag + 1
//! ```
//!
//! Objects start with a header word: kind (8 bits) | constructor tag (24 bits) | size (32 bits), followed by
//! the fields. Field i is at offset 8 * (i + 1).

use std::collections::{HashMap, HashSet};

use super::Symbols;
use super::x86::*;
use crate::builtins;
use crate::ir::{self, BinOp, BlockId, Function, Inst, Module, Term, UnOp, V};

pub const KIND_FIELDS: u64 = 0;
pub const KIND_STRING: u64 = 1;
pub const KIND_ARRAY: u64 = 2;
pub const KIND_CLOSURE: u64 = 3;

pub fn header(kind: u64, tag: u32, size: u32) -> u64 {
    kind | (u64::from(tag) << 8) | (u64::from(size) << 32)
}

pub fn tagged(n: i64) -> i64 {
    n.wrapping_mul(2).wrapping_add(1)
}

struct Sel<'a> {
    f: &'a Function,
    abi: Abi,
    syms: &'a Symbols,
    blocks: Vec<MBlock>,
    bmap: HashMap<BlockId, u32>,
    cur: u32,
    next_vreg: u32,
    div0: Option<u32>,
    fused: HashSet<V>,
    /// How many times each value is used.
    uses: HashMap<V, usize>,
    /// Values computed directly into the block parameter they are passed to (see `write_into_params`).
    rename: HashMap<V, V>,
    max_stack_args: u32,
    /// Optimize: divide by constants with multiplications, and inline array and string accesses.
    fast: bool,
    /// Shared out-of-line blocks that report a bad array or string index (taken from r10, length in rax).
    oob_array: Option<u32>,
    oob_string: Option<u32>,
}

pub fn select(m: &Module, fid: usize, abi: Abi, syms: &Symbols, fast: bool) -> MFunction {
    let f = &m.funcs[fid];
    let order = f.rpo();
    let mut s = Sel {
        f,
        abi,
        syms,
        blocks: Vec::new(),
        bmap: HashMap::new(),
        cur: 0,
        next_vreg: f.values.len() as u32,
        div0: None,
        fused: HashSet::new(),
        uses: HashMap::new(),
        rename: HashMap::new(),
        max_stack_args: 0,
        fast,
        oob_array: None,
        oob_string: None,
    };
    for (i, &b) in order.iter().enumerate() {
        s.bmap.insert(b, i as u32);
        s.blocks.push(MBlock::default());
    }
    s.find_fusable(&order);

    // Entry: move the arguments from where the calling convention puts them.
    let regs = abi.arg_regs();
    let moves: Vec<(Op, Op)> = f
        .params()
        .iter()
        .enumerate()
        .map(|(i, &p)| (Op::v(p), if i < regs.len() { Op::r(regs[i]) } else { Op::Incoming((i - regs.len()) as u32) }))
        .collect();
    if !moves.is_empty() {
        s.blocks[0].insts.push(MI::ParMove(moves));
    }
    for &b in &order {
        s.cur = s.bmap[&b];
        let blk = &f.blocks[b as usize];
        s.write_into_params(blk);
        for &v in &blk.insts {
            if !s.fused.contains(&v) {
                s.inst(v, f.inst(v).unwrap());
            }
        }
        s.term(b, &blk.term);
    }
    MFunction {
        name: f.name.clone(),
        symbol: syms.func(fid as u32),
        blocks: s.blocks,
        vregs: s.next_vreg,
        max_stack_args: s.max_stack_args,
    }
}

impl<'a> Sel<'a> {
    fn emit(&mut self, i: MI) {
        self.blocks[self.cur as usize].insts.push(i);
    }

    fn new_block(&mut self) -> u32 {
        self.blocks.push(MBlock::default());
        self.blocks.len() as u32 - 1
    }

    fn temp(&mut self) -> u32 {
        self.next_vreg += 1;
        self.next_vreg - 1
    }

    /// Comparisons whose only use is the branch at the end of their block are emitted as `cmp; jcc`.
    fn find_fusable(&mut self, order: &[BlockId]) {
        let mut uses: HashMap<V, usize> = HashMap::new();
        for &b in order {
            let blk = &self.f.blocks[b as usize];
            for &v in &blk.insts {
                for o in self.f.inst(v).unwrap().operands() {
                    *uses.entry(o).or_default() += 1;
                }
            }
            for o in blk.term.operands() {
                *uses.entry(o).or_default() += 1;
            }
        }
        for &b in order {
            let blk = &self.f.blocks[b as usize];
            if let Term::Branch(c, _, _) = &blk.term
                && uses.get(c) == Some(&1)
                && blk.insts.contains(c)
                && let Some(Inst::Binary(op, _, _)) = self.f.inst(*c)
                && op.is_compare()
            {
                self.fused.insert(*c);
            }
        }
        self.uses = uses;
    }

    fn div0_block(&mut self) -> u32 {
        if let Some(b) = self.div0 {
            return b;
        }
        let b = self.new_block();
        self.blocks[b as usize].insts.push(MI::CallNoReturn { sym: "quill_panic_div0".into(), args: vec![] });
        self.div0 = Some(b);
        b
    }

    /// The register a value is computed into: its own, or the block parameter it is passed to.
    fn dst_of(&self, v: V) -> Op {
        Op::v(self.rename.get(&v).copied().unwrap_or(v))
    }

    /// A value whose only use is as an argument of the block's final jump can be computed straight into
    /// the target's parameter, which saves a move (and a register) per loop iteration: `j = j + i` then
    /// updates j in place. That is safe when the old value of the parameter is not read after the new
    /// one is computed, and the instruction writes its result after reading all of its operands.
    fn write_into_params(&mut self, blk: &ir::Block) {
        self.rename.clear();
        let Term::Jump(t) = &blk.term else { return };
        let params = &self.f.blocks[t.block as usize].params;
        for (k, (&p, &a)) in params.iter().zip(&t.args).enumerate() {
            let Some(pos) = blk.insts.iter().position(|&x| x == a) else { continue };
            let simple = matches!(
                self.f.inst(a),
                Some(
                    Inst::Binary(..)
                        | Inst::Unary(..)
                        | Inst::Field(..)
                        | Inst::Tag(_)
                        | Inst::Call(..)
                        | Inst::CallClosure(..)
                        | Inst::CallBuiltin(..)
                )
            );
            let read_later = blk.insts[pos + 1..].iter().any(|&x| self.f.inst(x).unwrap().operands().contains(&p))
                || t.args.iter().enumerate().any(|(j, &o)| j != k && o == p);
            if a != p && simple && self.uses.get(&a) == Some(&1) && !self.fused.contains(&a) && !read_later {
                self.rename.insert(a, p);
            }
        }
    }

    /// The block that reports an out-of-range index: index in r10, length in rax (both untagged).
    fn oob_block(&mut self, string: bool) -> u32 {
        let slot = if string { self.oob_string } else { self.oob_array };
        if let Some(b) = slot {
            return b;
        }
        let b = self.new_block();
        let sym = if string { "quill_string_index_error" } else { "quill_array_index_error" };
        self.blocks[b as usize].insts.push(MI::CallNoReturn { sym: sym.into(), args: vec![Op::r(Reg::R10), Op::r(Reg::Rax)] });
        if string {
            self.oob_string = Some(b);
        } else {
            self.oob_array = Some(b);
        }
        b
    }

    /// Leaves the untagged index in r10 and jumps out if it is not below the length of the object.
    fn bounds_check(&mut self, obj: V, index: V, string: bool) {
        let (r10, rax) = (Op::r(Reg::R10), Op::r(Reg::Rax));
        self.emit(MI::Mov(r10.clone(), self.val(index)));
        self.emit(MI::Shift(Shift::Sar, r10.clone(), 1));
        self.emit(MI::Mov(rax.clone(), Op::Mem(Loc::V(obj), 0)));
        self.emit(MI::Shift(Shift::Shr, rax.clone(), 32));
        self.emit(MI::Alu(Alu::Cmp, r10, rax));
        let fail = self.oob_block(string);
        self.emit(MI::Jcc(Cond::Ae, fail));
    }

    /// Array and string builtins as inline code. Returns false for the ones that stay calls.
    fn inline_builtin(&mut self, v: V, b: builtins::Builtin, args: &[V]) -> bool {
        let dst = self.dst_of(v);
        use builtins::Builtin::*;
        let r10 = Op::r(Reg::R10);
        match b {
            ArrayLength | StringLength => {
                // The length is the size field of the header.
                self.emit(MI::Mov(r10.clone(), Op::Mem(Loc::V(args[0]), 0)));
                self.emit(MI::Shift(Shift::Shr, r10, 32));
                self.emit(MI::Lea { dst, base: Loc::R(Reg::R10), index: Some(Loc::R(Reg::R10)), scale: 1, disp: 1 });
            }
            ArrayGet => {
                self.bounds_check(args[0], args[1], false);
                self.emit(MI::Load { dst, base: Loc::V(args[0]), index: Loc::R(Reg::R10), scale: 8, disp: 8, byte: false });
            }
            ArraySet => {
                self.bounds_check(args[0], args[1], false);
                let src = self.val(args[2]);
                self.emit(MI::Store { base: Loc::V(args[0]), index: Loc::R(Reg::R10), scale: 8, disp: 8, src });
                if self.uses.contains_key(&v) {
                    self.emit(MI::Mov(dst, Op::Imm(tagged(0))));
                }
            }
            StringGet => {
                self.bounds_check(args[0], args[1], true);
                let rax = Op::r(Reg::Rax);
                self.emit(MI::Load { dst: rax, base: Loc::V(args[0]), index: Loc::R(Reg::R10), scale: 1, disp: 8, byte: true });
                self.emit(MI::Lea { dst, base: Loc::R(Reg::Rax), index: Some(Loc::R(Reg::Rax)), scale: 1, disp: 1 });
            }
            _ => return false,
        }
        true
    }

    fn konst(&self, v: V) -> Option<i64> {
        match self.f.inst(v) {
            Some(Inst::Const(k)) => Some(*k),
            _ => None,
        }
    }

    /// The operand for a value: an immediate for small constants, otherwise its virtual register.
    fn val(&self, v: V) -> Op {
        match self.konst(v) {
            Some(k) if i32::try_from(tagged(k)).is_ok() => Op::Imm(tagged(k)),
            _ => Op::v(v),
        }
    }

    fn inst(&mut self, v: V, inst: &Inst) {
        let r10 = Op::r(Reg::R10);
        let rax = Op::r(Reg::Rax);
        let dst = self.dst_of(v);
        match inst {
            // Every use of a constant that fits a 32-bit immediate takes it as one (see `val`).
            Inst::Const(k) if i32::try_from(tagged(*k)).is_ok() => {}
            Inst::Const(k) => self.emit(MI::Mov(dst, Op::Imm(tagged(*k)))),
            Inst::Str(s) => self.emit(MI::LeaSym(dst, self.syms.string(*s))),
            Inst::Ctor0(c) => self.emit(MI::LeaSym(dst, self.syms.ctor0(*c))),
            Inst::FuncRef(g) => self.emit(MI::LeaSym(dst, self.syms.closure(*g))),
            Inst::Unary(UnOp::Neg, a) => {
                // -(2x+1) + 2 = 2(-x)+1
                self.emit(MI::Mov(r10.clone(), Op::Imm(2)));
                self.emit(MI::Alu(Alu::Sub, r10.clone(), self.val(*a)));
                self.emit(MI::Mov(dst, r10));
            }
            Inst::Unary(UnOp::Not, a) => {
                // 1 (false) <-> 3 (true)
                self.emit(MI::Mov(r10.clone(), self.val(*a)));
                self.emit(MI::Alu(Alu::Xor, r10.clone(), Op::Imm(2)));
                self.emit(MI::Mov(dst, r10));
            }
            Inst::Binary(op, a, b) => self.binary(dst, *op, *a, *b),
            Inst::Call(g, args) => {
                let args: Vec<Op> = args.iter().map(|&a| self.val(a)).collect();
                self.note_args(args.len());
                self.emit(MI::Call { target: CallTarget::Sym(self.syms.func(*g)), args, ret: Some(dst) });
            }
            Inst::CallClosure(c, args) => {
                let all: Vec<Op> = std::iter::once(Op::v(*c)).chain(args.iter().map(|&a| self.val(a))).collect();
                self.note_args(all.len());
                self.emit(MI::Call { target: CallTarget::Closure(Op::v(*c)), args: all, ret: Some(dst) });
            }
            Inst::CallBuiltin(b, args) if self.fast && self.inline_builtin(v, *b, args) => {}
            Inst::CallBuiltin(b, args) => {
                let args: Vec<Op> = args.iter().map(|&a| self.val(a)).collect();
                self.note_args(args.len());
                self.emit(MI::Call { target: CallTarget::Sym(builtins::info(*b).symbol.to_string()), args, ret: Some(dst) });
            }
            Inst::Alloc { tag, fields } => {
                self.alloc(v, header(KIND_FIELDS, *tag, fields.len() as u32), fields.len());
                for (i, &fv) in fields.iter().enumerate() {
                    self.store_field(v, i, fv);
                }
            }
            Inst::MakeClosure { func, captures } => {
                self.alloc(v, header(KIND_CLOSURE, 0, captures.len() as u32 + 1), captures.len() + 1);
                self.emit(MI::LeaSym(r10.clone(), self.syms.func(*func)));
                self.emit(MI::Mov(Op::Mem(Loc::V(v), 8), r10));
                for (i, &cv) in captures.iter().enumerate() {
                    self.store_field(v, i + 1, cv);
                }
            }
            Inst::Field(o, i) => self.emit(MI::Mov(dst, Op::Mem(Loc::V(*o), 8 * (*i as i32 + 1)))),
            Inst::Tag(o) => {
                self.emit(MI::Mov(r10.clone(), Op::Mem(Loc::V(*o), 0)));
                self.emit(MI::Shift(Shift::Shr, r10.clone(), 8));
                self.emit(MI::Alu(Alu::And, r10.clone(), Op::Imm(0xFF_FFFF)));
                self.emit(MI::Lea { dst: dst.clone(), base: Loc::R(Reg::R10), index: Some(Loc::R(Reg::R10)), scale: 1, disp: 1 });
                let _ = rax;
            }
        }
    }

    fn note_args(&mut self, n: usize) {
        let regs = self.abi.arg_regs().len();
        self.max_stack_args = self.max_stack_args.max(n.saturating_sub(regs) as u32);
    }

    fn alloc(&mut self, v: V, header: u64, nfields: usize) {
        self.note_args(2);
        self.emit(MI::Call {
            target: CallTarget::Sym("quill_alloc".into()),
            args: vec![Op::Imm(nfields as i64), Op::Imm(header as i64)],
            ret: Some(Op::v(v)),
        });
    }

    fn store_field(&mut self, obj: V, i: usize, val: V) {
        let src = self.val(val);
        self.emit(MI::Mov(Op::Mem(Loc::V(obj), 8 * (i as i32 + 1)), src));
    }

    fn binary(&mut self, dst: Op, op: BinOp, a: V, b: V) {
        let r10 = Op::r(Reg::R10);
        let rax = Op::r(Reg::Rax);
        let rcx = Op::r(Reg::Rcx);
        let small = |k: i64| i32::try_from(k).is_ok();
        // Put a constant on the right of commutative operations and comparisons.
        let (a, b, op) = if self.konst(a).is_some() && self.konst(b).is_none() {
            match op {
                BinOp::Add | BinOp::Mul | BinOp::Eq | BinOp::Ne => (b, a, op),
                BinOp::Lt => (b, a, BinOp::Gt),
                BinOp::Le => (b, a, BinOp::Ge),
                BinOp::Gt => (b, a, BinOp::Lt),
                BinOp::Ge => (b, a, BinOp::Le),
                _ => (a, b, op),
            }
        } else {
            (a, b, op)
        };
        let a_reg = self.konst(a).is_none();
        match op {
            BinOp::Add => match self.konst(b) {
                // (2x+1) + 2k
                Some(k) if a_reg && small(k.wrapping_mul(2)) => {
                    self.emit(MI::Lea { dst, base: Loc::V(a), index: None, scale: 1, disp: (k * 2) as i32 })
                }
                // (2x+1) + (2y+1) - 1
                None if a_reg => self.emit(MI::Lea { dst, base: Loc::V(a), index: Some(Loc::V(b)), scale: 1, disp: -1 }),
                _ => {
                    self.emit(MI::Mov(r10.clone(), self.val(a)));
                    self.emit(MI::Alu(Alu::Add, r10.clone(), self.val(b)));
                    self.emit(MI::Lea { dst, base: Loc::R(Reg::R10), index: None, scale: 1, disp: -1 });
                }
            },
            BinOp::Sub => match self.konst(b) {
                Some(k) if a_reg && small(k.wrapping_mul(2)) => {
                    self.emit(MI::Lea { dst, base: Loc::V(a), index: None, scale: 1, disp: -(k * 2) as i32 })
                }
                _ => {
                    self.emit(MI::Mov(r10.clone(), self.val(a)));
                    self.emit(MI::Alu(Alu::Sub, r10.clone(), self.val(b)));
                    self.emit(MI::Lea { dst, base: Loc::R(Reg::R10), index: None, scale: 1, disp: 1 });
                }
            },
            BinOp::Mul => match self.konst(b) {
                // (2x+1) * k - (k - 1) = 2xk + 1
                Some(k) if a_reg && small(k) && small(k - 1) => {
                    self.emit(MI::IMul3(dst.clone(), Op::v(a), k as i32));
                    if k != 1 {
                        self.emit(MI::Alu(Alu::Sub, dst, Op::Imm(k - 1)));
                    }
                }
                _ => {
                    // (a >> 1) * (b - 1) + 1
                    self.emit(MI::Mov(r10.clone(), self.val(a)));
                    self.emit(MI::Shift(Shift::Sar, r10.clone(), 1));
                    self.emit(MI::Mov(rax.clone(), self.val(b)));
                    self.emit(MI::Alu(Alu::Sub, rax.clone(), Op::Imm(1)));
                    self.emit(MI::Alu(Alu::IMul, r10.clone(), rax));
                    self.emit(MI::Lea { dst, base: Loc::R(Reg::R10), index: None, scale: 1, disp: 1 });
                }
            },
            BinOp::Div | BinOp::Mod => {
                if let Some(p) = self.konst(b).filter(|&k| k > 1 && (k & (k - 1)) == 0).map(|k| k.trailing_zeros() as u8) {
                    // Signed division by 2^p without idiv: add 2^p - 1 to negative dividends, then shift.
                    self.emit(MI::Mov(rax.clone(), self.val(a)));
                    self.emit(MI::Shift(Shift::Sar, rax.clone(), 1));
                    self.emit(MI::Mov(rcx.clone(), rax.clone()));
                    self.emit(MI::Shift(Shift::Sar, rcx.clone(), 63));
                    self.emit(MI::Shift(Shift::Shr, rcx.clone(), 64 - p));
                    self.emit(MI::Alu(Alu::Add, rcx.clone(), rax.clone()));
                    if op == BinOp::Div {
                        self.emit(MI::Shift(Shift::Sar, rcx.clone(), p));
                        self.emit(MI::Lea { dst, base: Loc::R(Reg::Rcx), index: Some(Loc::R(Reg::Rcx)), scale: 1, disp: 1 });
                    } else {
                        self.emit(MI::Alu(Alu::And, rcx.clone(), Op::Imm(-(1i64 << p))));
                        self.emit(MI::Alu(Alu::Sub, rax.clone(), rcx));
                        self.emit(MI::Lea { dst, base: Loc::R(Reg::Rax), index: Some(Loc::R(Reg::Rax)), scale: 1, disp: 1 });
                    }
                    return;
                }
                if let Some(d) = self.konst(b).filter(|&d| self.fast && !(-1..=1).contains(&d)) {
                    self.divide_by_constant(dst, op, a, d);
                    return;
                }
                self.emit(MI::Mov(rax.clone(), self.val(a)));
                self.emit(MI::Shift(Shift::Sar, rax.clone(), 1));
                self.emit(MI::Mov(rcx.clone(), self.val(b)));
                self.emit(MI::Shift(Shift::Sar, rcx.clone(), 1));
                if self.konst(b).is_none_or(|k| k == 0) {
                    let d0 = self.div0_block();
                    self.emit(MI::Alu(Alu::Test, rcx.clone(), rcx.clone()));
                    self.emit(MI::Jcc(Cond::E, d0));
                }
                self.emit(MI::Cqo);
                self.emit(MI::IDiv(rcx));
                let res = if op == BinOp::Div { Reg::Rax } else { Reg::Rdx };
                self.emit(MI::Lea { dst, base: Loc::R(res), index: Some(Loc::R(res)), scale: 1, disp: 1 });
            }
            _ => {
                // Tagging preserves order, so tagged values compare directly.
                self.emit(MI::Alu(Alu::Cmp, self.val(a), self.val(b)));
                self.emit(MI::SetCC(cond_of(op)));
                self.emit(MI::Lea { dst, base: Loc::R(Reg::Rax), index: Some(Loc::R(Reg::Rax)), scale: 1, disp: 1 });
            }
        }
    }

    /// `a / d` and `a % d` (truncating) for a constant d with |d| >= 2, by multiplying with a magic
    /// number instead of dividing (Hacker's Delight, chapter 10).
    fn divide_by_constant(&mut self, dst: Op, op: BinOp, a: V, d: i64) {
        let (rax, rcx, rdx) = (Op::r(Reg::Rax), Op::r(Reg::Rcx), Op::r(Reg::Rdx));
        let (m, shift) = magic(d);
        // n in rcx; q = hi(m * n) (+/- n), shifted, plus one if negative.
        self.emit(MI::Mov(rcx.clone(), self.val(a)));
        self.emit(MI::Shift(Shift::Sar, rcx.clone(), 1));
        self.emit(MI::Mov(rax.clone(), Op::Imm(m)));
        self.emit(MI::IMulWide(rcx.clone()));
        if d > 0 && m < 0 {
            self.emit(MI::Alu(Alu::Add, rdx.clone(), rcx.clone()));
        } else if d < 0 && m > 0 {
            self.emit(MI::Alu(Alu::Sub, rdx.clone(), rcx.clone()));
        }
        if shift > 0 {
            self.emit(MI::Shift(Shift::Sar, rdx.clone(), shift as u8));
        }
        self.emit(MI::Mov(rax.clone(), rdx.clone()));
        self.emit(MI::Shift(Shift::Shr, rax.clone(), 63));
        self.emit(MI::Alu(Alu::Add, rdx.clone(), rax.clone()));
        if op == BinOp::Div {
            self.emit(MI::Lea { dst, base: Loc::R(Reg::Rdx), index: Some(Loc::R(Reg::Rdx)), scale: 1, disp: 1 });
        } else {
            // n - q * d
            self.emit(MI::Mov(rax.clone(), Op::Imm(d)));
            self.emit(MI::Alu(Alu::IMul, rdx.clone(), rax));
            self.emit(MI::Alu(Alu::Sub, rcx, rdx));
            self.emit(MI::Lea { dst, base: Loc::R(Reg::Rcx), index: Some(Loc::R(Reg::Rcx)), scale: 1, disp: 1 });
        }
    }

    fn term(&mut self, b: BlockId, t: &Term) {
        match t {
            Term::Jump(target) => {
                self.jump_with_args(target);
            }
            Term::Branch(c, t, e) => {
                let tb = self.edge(t);
                let eb = self.edge(e);
                if self.fused.contains(c) {
                    let Some(Inst::Binary(op, x, y)) = self.f.inst(*c).cloned() else { unreachable!() };
                    let (x, y, op) = if self.konst(x).is_some() && self.konst(y).is_none() { (y, x, swap_cmp(op)) } else { (x, y, op) };
                    self.emit(MI::Alu(Alu::Cmp, self.val(x), self.val(y)));
                    self.emit(MI::Jcc(cond_of(op), tb));
                } else {
                    // false is tagged 1
                    self.emit(MI::Alu(Alu::Cmp, self.val(*c), Op::Imm(1)));
                    self.emit(MI::Jcc(Cond::Ne, tb));
                }
                self.emit(MI::Jmp(eb));
                let _ = b;
            }
            Term::Return(v) => self.emit(MI::Ret(self.val(*v))),
            Term::Panic(s) => {
                let r = self.temp();
                self.emit(MI::LeaSym(Op::v(r), self.syms.string(*s)));
                self.emit(MI::CallNoReturn { sym: "quill_panic".into(), args: vec![Op::v(r)] });
            }
            Term::Unreachable => self.emit(MI::CallNoReturn { sym: "quill_unreachable".into(), args: vec![] }),
        }
    }

    fn jump_with_args(&mut self, t: &ir::Target) {
        let params = &self.f.blocks[t.block as usize].params;
        let moves: Vec<(Op, Op)> = params
            .iter()
            .zip(&t.args)
            .filter(|&(p, a)| p != a && self.rename.get(a) != Some(p))
            .map(|(&p, &a)| (Op::v(p), self.val(a)))
            .collect();
        if !moves.is_empty() {
            self.emit(MI::ParMove(moves));
        }
        self.emit(MI::Jmp(self.bmap[&t.block]));
    }

    /// The machine block to branch to for a target: the target itself, or a new block that first moves
    /// the arguments into the target's parameters.
    fn edge(&mut self, t: &ir::Target) -> u32 {
        if t.args.is_empty() {
            return self.bmap[&t.block];
        }
        let saved = self.cur;
        let e = self.new_block();
        self.cur = e;
        self.jump_with_args(t);
        self.cur = saved;
        e
    }
}

/// The comparison with its operands exchanged: `a < b` is `b > a`.
fn swap_cmp(op: BinOp) -> BinOp {
    match op {
        BinOp::Lt => BinOp::Gt,
        BinOp::Le => BinOp::Ge,
        BinOp::Gt => BinOp::Lt,
        BinOp::Ge => BinOp::Le,
        other => other,
    }
}

fn cond_of(op: BinOp) -> Cond {
    match op {
        BinOp::Lt => Cond::L,
        BinOp::Le => Cond::Le,
        BinOp::Gt => Cond::G,
        BinOp::Ge => Cond::Ge,
        BinOp::Eq => Cond::E,
        BinOp::Ne => Cond::Ne,
        _ => unreachable!("not a comparison"),
    }
}

/// The magic multiplier and shift for signed 64-bit division by d (|d| >= 2): `n / d` is
/// `hi(m * n) (+ n if d > 0 and m < 0, - n if d < 0 and m > 0) >> shift`, plus one if that is negative.
pub fn magic(d: i64) -> (i64, u32) {
    const TWO63: u64 = 1 << 63;
    let ad = d.unsigned_abs();
    let t = TWO63 + ((d as u64) >> 63);
    let anc = t - 1 - t % ad;
    let mut p = 63;
    let mut q1 = TWO63 / anc;
    let mut r1 = TWO63 - q1 * anc;
    let mut q2 = TWO63 / ad;
    let mut r2 = TWO63 - q2 * ad;
    loop {
        p += 1;
        q1 = q1.wrapping_mul(2);
        r1 = r1.wrapping_mul(2);
        if r1 >= anc {
            q1 = q1.wrapping_add(1);
            r1 = r1.wrapping_sub(anc);
        }
        q2 = q2.wrapping_mul(2);
        r2 = r2.wrapping_mul(2);
        if r2 >= ad {
            q2 = q2.wrapping_add(1);
            r2 = r2.wrapping_sub(ad);
        }
        let delta = ad - r2;
        if !(q1 < delta || (q1 == delta && r1 == 0)) {
            break;
        }
    }
    let m = q2.wrapping_add(1) as i64;
    (if d < 0 { m.wrapping_neg() } else { m }, p - 64)
}

#[cfg(test)]
mod tests {
    use super::magic;

    /// The instruction sequence of `divide_by_constant`, evaluated in Rust.
    fn quotient(n: i64, d: i64) -> i64 {
        let (m, s) = magic(d);
        let mut q = ((i128::from(m) * i128::from(n)) >> 64) as i64;
        if d > 0 && m < 0 {
            q = q.wrapping_add(n);
        } else if d < 0 && m > 0 {
            q = q.wrapping_sub(n);
        }
        q >>= s;
        q + ((q as u64) >> 63) as i64
    }

    #[test]
    fn magic_division_matches_idiv() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let lim = 1i64 << 62;
        let mut divisors: Vec<i64> = vec![2, 3, 5, 6, 7, 10, 12, 25, 100, 641, 1000, 1 << 20, lim - 1, 3 << 40];
        divisors.extend(divisors.clone().iter().map(|d| -d));
        divisors.push(-lim);
        for _ in 0..200 {
            let d = (next() as i64) >> (2 + next() % 60);
            if !(-1..=1).contains(&d) {
                divisors.push(d);
            }
        }
        for &d in &divisors {
            let mut ns = vec![0, 1, -1, lim - 1, -lim, d, -d, d - 1, d + 1];
            ns.extend((0..300).map(|_| (next() as i64) >> 2 >> (next() % 62)));
            for n in ns {
                assert_eq!(quotient(n, d), n / d, "{n} / {d}");
            }
        }
    }
}
