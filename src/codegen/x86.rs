//! x86-64 registers, calling conventions and the machine IR (MIR) that instruction selection produces and
//! register allocation consumes.

use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reg {
    Rax,
    Rcx,
    Rdx,
    Rbx,
    Rsp,
    Rbp,
    Rsi,
    Rdi,
    R8,
    R9,
    R10,
    R11,
    R12,
    R13,
    R14,
    R15,
}

impl Reg {
    pub fn name(self) -> &'static str {
        match self {
            Reg::Rax => "rax",
            Reg::Rcx => "rcx",
            Reg::Rdx => "rdx",
            Reg::Rbx => "rbx",
            Reg::Rsp => "rsp",
            Reg::Rbp => "rbp",
            Reg::Rsi => "rsi",
            Reg::Rdi => "rdi",
            Reg::R8 => "r8",
            Reg::R9 => "r9",
            Reg::R10 => "r10",
            Reg::R11 => "r11",
            Reg::R12 => "r12",
            Reg::R13 => "r13",
            Reg::R14 => "r14",
            Reg::R15 => "r15",
        }
    }

    /// The low 32 bits of the register.
    pub fn name32(self) -> &'static str {
        match self {
            Reg::Rax => "eax",
            Reg::Rcx => "ecx",
            Reg::Rdx => "edx",
            Reg::Rbx => "ebx",
            Reg::Rsp => "esp",
            Reg::Rbp => "ebp",
            Reg::Rsi => "esi",
            Reg::Rdi => "edi",
            Reg::R8 => "r8d",
            Reg::R9 => "r9d",
            Reg::R10 => "r10d",
            Reg::R11 => "r11d",
            Reg::R12 => "r12d",
            Reg::R13 => "r13d",
            Reg::R14 => "r14d",
            Reg::R15 => "r15d",
        }
    }
}

impl fmt::Display for Reg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The calling convention of the target platform. Quill functions use it too, so that they can call and
/// be called by the C run-time library directly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Abi {
    /// Windows x64: arguments in rcx, rdx, r8, r9, 32 bytes of shadow space; rbx, rbp, rdi, rsi, r12-r15
    /// are preserved by calls.
    Win64,
    /// System V AMD64 (Linux, macOS): arguments in rdi, rsi, rdx, rcx, r8, r9; rbx, rbp, r12-r15 preserved.
    SysV,
}

impl Abi {
    pub fn host() -> Abi {
        if cfg!(windows) { Abi::Win64 } else { Abi::SysV }
    }

    pub fn arg_regs(self) -> &'static [Reg] {
        match self {
            Abi::Win64 => &[Reg::Rcx, Reg::Rdx, Reg::R8, Reg::R9],
            Abi::SysV => &[Reg::Rdi, Reg::Rsi, Reg::Rdx, Reg::Rcx, Reg::R8, Reg::R9],
        }
    }

    /// Bytes the caller reserves above the return address before stack arguments.
    pub fn shadow_space(self) -> i32 {
        match self {
            Abi::Win64 => 32,
            Abi::SysV => 0,
        }
    }

    pub fn is_callee_saved(self, r: Reg) -> bool {
        match self {
            Abi::Win64 => matches!(r, Reg::Rbx | Reg::Rsi | Reg::Rdi | Reg::R12 | Reg::R13 | Reg::R14 | Reg::R15),
            Abi::SysV => matches!(r, Reg::Rbx | Reg::R12 | Reg::R13 | Reg::R14 | Reg::R15),
        }
    }

    /// Registers the allocator may hand out. rax, rcx, rdx, r10 and r11 are kept as scratch registers
    /// for instruction sequences (division, shifts, calls, spill code), rsp and rbp for the frame.
    pub fn allocatable(self) -> Vec<Reg> {
        // Caller-saved first: they are free to use for values that do not live across a call.
        let mut regs: Vec<Reg> = [Reg::R8, Reg::R9, Reg::Rsi, Reg::Rdi, Reg::Rbx, Reg::R12, Reg::R13, Reg::R14, Reg::R15].to_vec();
        regs.sort_by_key(|r| self.is_callee_saved(*r));
        regs
    }
}

/// A virtual or physical register.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Loc {
    V(u32),
    R(Reg),
}

/// An operand.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    L(Loc),
    Imm(i64),
    /// `[base + disp]`
    Mem(Loc, i32),
    /// A spill slot (only after register allocation).
    Slot(u32),
    /// The i-th incoming argument passed on the stack.
    Incoming(u32),
    /// The i-th outgoing argument passed on the stack (in the call's argument area).
    Outgoing(u32),
}

impl Op {
    pub fn v(v: u32) -> Op {
        Op::L(Loc::V(v))
    }

    pub fn r(r: Reg) -> Op {
        Op::L(Loc::R(r))
    }

    pub fn is_mem(&self) -> bool {
        matches!(self, Op::Mem(..) | Op::Slot(_) | Op::Incoming(_) | Op::Outgoing(_))
    }

    pub fn reg(&self) -> Option<Reg> {
        match self {
            Op::L(Loc::R(r)) => Some(*r),
            _ => None,
        }
    }

    /// Virtual registers read by this operand when it is used as a source (or, for memory operands,
    /// also when used as a destination: the base is read).
    pub fn vregs(&self) -> Vec<u32> {
        match self {
            Op::L(Loc::V(v)) | Op::Mem(Loc::V(v), _) => vec![*v],
            _ => vec![],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alu {
    Add,
    Sub,
    And,
    Or,
    Xor,
    IMul,
    Cmp,
    Test,
}

impl Alu {
    pub fn name(self) -> &'static str {
        match self {
            Alu::Add => "add",
            Alu::Sub => "sub",
            Alu::And => "and",
            Alu::Or => "or",
            Alu::Xor => "xor",
            Alu::IMul => "imul",
            Alu::Cmp => "cmp",
            Alu::Test => "test",
        }
    }

    /// Does the instruction write its first operand?
    pub fn writes_dst(self) -> bool {
        !matches!(self, Alu::Cmp | Alu::Test)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Sar,
    Shr,
    Shl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cond {
    E,
    Ne,
    L,
    Le,
    G,
    Ge,
    /// Unsigned `>=` (bounds checks: a negative index is a huge unsigned one).
    Ae,
}

impl Cond {
    pub fn name(self) -> &'static str {
        match self {
            Cond::E => "e",
            Cond::Ne => "ne",
            Cond::L => "l",
            Cond::Le => "le",
            Cond::G => "g",
            Cond::Ge => "ge",
            Cond::Ae => "ae",
        }
    }

    pub fn negate(self) -> Cond {
        match self {
            Cond::E => Cond::Ne,
            Cond::Ne => Cond::E,
            Cond::L => Cond::Ge,
            Cond::Le => Cond::G,
            Cond::G => Cond::Le,
            Cond::Ge => Cond::L,
            Cond::Ae => unreachable!("only used for bounds checks"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum CallTarget {
    Sym(String),
    /// Call the code pointer stored in field 0 of this closure object (`[closure + 8]`).
    Closure(Op),
}

/// Machine instructions. A few are pseudo-instructions that are expanded after register allocation:
/// `Call` (argument passing), `Ret` (epilogue), `ParMove` (parallel copies for block arguments).
#[derive(Clone, Debug, PartialEq)]
pub enum MI {
    Mov(Op, Op),
    /// `lea dst, [base + index*scale + disp]`
    Lea {
        dst: Op,
        base: Loc,
        index: Option<Loc>,
        scale: u8,
        disp: i32,
    },
    /// `lea dst, [rip + sym]`
    LeaSym(Op, String),
    Alu(Alu, Op, Op),
    /// `imul dst, src, imm`
    IMul3(Op, Op, i32),
    Shift(Shift, Op, u8),
    Neg(Op),
    /// `setcc al; movzx eax, al`
    SetCC(Cond),
    Cqo,
    IDiv(Op),
    /// One-operand `imul src`: rdx:rax = rax * src.
    IMulWide(Op),
    /// `mov dst, [base + index*scale + disp]` (`byte` zero-extends a single byte).
    Load {
        dst: Op,
        base: Loc,
        index: Loc,
        scale: u8,
        disp: i32,
        byte: bool,
    },
    /// `mov [base + index*scale + disp], src`
    Store {
        base: Loc,
        index: Loc,
        scale: u8,
        disp: i32,
        src: Op,
    },
    Jmp(u32),
    Jcc(Cond, u32),
    Call {
        target: CallTarget,
        args: Vec<Op>,
        ret: Option<Op>,
    },
    Ret(Op),
    ParMove(Vec<(Op, Op)>),
    /// A call that never returns (run-time errors): no argument shuffling needed.
    CallNoReturn {
        sym: String,
        args: Vec<Op>,
    },
}

impl MI {
    /// (uses, defs) in terms of virtual registers.
    pub fn uses_defs(&self) -> (Vec<u32>, Vec<u32>) {
        let mut uses = Vec::new();
        let mut defs = Vec::new();
        let dst = |o: &Op, uses: &mut Vec<u32>, defs: &mut Vec<u32>| match o {
            Op::L(Loc::V(v)) => defs.push(*v),
            Op::Mem(Loc::V(v), _) => uses.push(*v),
            _ => {}
        };
        match self {
            MI::Mov(d, s) => {
                uses.extend(s.vregs());
                dst(d, &mut uses, &mut defs);
            }
            MI::Lea { dst: d, base, index, .. } => {
                if let Loc::V(v) = base {
                    uses.push(*v);
                }
                if let Some(Loc::V(v)) = index {
                    uses.push(*v);
                }
                dst(d, &mut uses, &mut defs);
            }
            MI::LeaSym(d, _) => dst(d, &mut uses, &mut defs),
            MI::Alu(op, d, s) => {
                uses.extend(s.vregs());
                uses.extend(d.vregs());
                if op.writes_dst() {
                    dst(d, &mut uses, &mut defs);
                }
            }
            MI::IMul3(d, s, _) => {
                uses.extend(s.vregs());
                dst(d, &mut uses, &mut defs);
            }
            MI::Shift(_, d, _) | MI::Neg(d) => {
                uses.extend(d.vregs());
                dst(d, &mut uses, &mut defs);
            }
            MI::IDiv(s) | MI::IMulWide(s) => uses.extend(s.vregs()),
            MI::Load { dst: d, base, index, .. } => {
                uses.extend([base, index].into_iter().filter_map(|l| if let Loc::V(v) = l { Some(*v) } else { None }));
                dst(d, &mut uses, &mut defs);
            }
            MI::Store { base, index, src, .. } => {
                uses.extend([base, index].into_iter().filter_map(|l| if let Loc::V(v) = l { Some(*v) } else { None }));
                uses.extend(src.vregs());
            }
            MI::Call { target, args, ret } => {
                if let CallTarget::Closure(c) = target {
                    uses.extend(c.vregs());
                }
                for a in args {
                    uses.extend(a.vregs());
                }
                if let Some(r) = ret {
                    dst(r, &mut uses, &mut defs);
                }
            }
            MI::Ret(v) => uses.extend(v.vregs()),
            MI::ParMove(moves) => {
                for (d, s) in moves {
                    uses.extend(s.vregs());
                    dst(d, &mut uses, &mut defs);
                }
            }
            MI::CallNoReturn { args, .. } => {
                for a in args {
                    uses.extend(a.vregs());
                }
            }
            MI::SetCC(_) | MI::Cqo | MI::Jmp(_) | MI::Jcc(..) => {}
        }
        (uses, defs)
    }

    pub fn map_ops(&mut self, f: &mut impl FnMut(&Op) -> Op, fl: &mut impl FnMut(Loc) -> Loc) {
        match self {
            MI::Mov(d, s) => {
                *d = f(d);
                *s = f(s);
            }
            MI::Lea { dst, base, index, .. } => {
                *dst = f(dst);
                *base = fl(*base);
                if let Some(i) = index {
                    *i = fl(*i);
                }
            }
            MI::LeaSym(d, _) | MI::Shift(_, d, _) | MI::Neg(d) | MI::IDiv(d) | MI::IMulWide(d) | MI::Ret(d) => *d = f(d),
            MI::Load { dst, base, index, .. } => {
                *dst = f(dst);
                *base = fl(*base);
                *index = fl(*index);
            }
            MI::Store { base, index, src, .. } => {
                *base = fl(*base);
                *index = fl(*index);
                *src = f(src);
            }
            MI::Alu(_, d, s) => {
                *d = f(d);
                *s = f(s);
            }
            MI::IMul3(d, s, _) => {
                *d = f(d);
                *s = f(s);
            }
            MI::Call { target, args, ret } => {
                if let CallTarget::Closure(c) = target {
                    *c = f(c);
                }
                for a in args.iter_mut() {
                    *a = f(a);
                }
                if let Some(r) = ret {
                    *r = f(r);
                }
            }
            MI::ParMove(moves) => {
                for (d, s) in moves.iter_mut() {
                    *d = f(d);
                    *s = f(s);
                }
            }
            MI::CallNoReturn { args, .. } => {
                for a in args.iter_mut() {
                    *a = f(a);
                }
            }
            MI::SetCC(_) | MI::Cqo | MI::Jmp(_) | MI::Jcc(..) => {}
        }
    }

    pub fn is_call(&self) -> bool {
        matches!(self, MI::Call { .. })
    }

    /// Blocks control may continue in after this instruction (besides falling through).
    pub fn branch_targets(&self) -> Vec<u32> {
        match self {
            MI::Jmp(b) | MI::Jcc(_, b) => vec![*b],
            _ => vec![],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct MBlock {
    pub insts: Vec<MI>,
}

/// A function in machine IR. Blocks are laid out in order; control falls through from a block to the
/// next only if it does not end in an unconditional jump, return or no-return call.
#[derive(Clone, Debug)]
pub struct MFunction {
    pub name: String,
    pub symbol: String,
    pub blocks: Vec<MBlock>,
    /// Number of virtual registers.
    pub vregs: u32,
    /// Largest number of stack-passed arguments of any call.
    pub max_stack_args: u32,
}

impl MFunction {
    /// Successor blocks of block `b` (explicit jumps plus the fall-through).
    pub fn successors(&self, b: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let blk = &self.blocks[b];
        for i in &blk.insts {
            for t in i.branch_targets() {
                out.push(t as usize);
            }
        }
        let falls = !matches!(blk.insts.last(), Some(MI::Jmp(_)) | Some(MI::Ret(_)) | Some(MI::CallNoReturn { .. }));
        if falls && b + 1 < self.blocks.len() {
            out.push(b + 1);
        }
        out.sort();
        out.dedup();
        out
    }
}
