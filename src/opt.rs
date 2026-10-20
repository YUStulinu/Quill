//! Optimizations on the SSA IR.
//!
//! * `simplify_cfg`: removes block parameters that always receive the same value, forwards jumps through
//!   empty blocks, merges blocks with a single predecessor, folds branches on constants.
//! * `sccp`: sparse conditional constant propagation (Wegman–Zadeck): constants flow through block
//!   parameters, and only edges that can actually execute are followed, so a branch decided by a
//!   constant also kills the code it skips.
//! * `instcombine`: algebraic identities (`x + 0`, `x * 1`, `!!x`, ...), loads from objects built in the
//!   same function, and devirtualization of calls through known closures.
//! * `gvn`: common subexpression elimination over the dominator tree.
//! * `dce`: removes instructions and block parameters whose values are never used.
//! * `inline`: inlines small non-recursive functions at direct call sites.

use std::collections::{HashMap, HashSet};

use crate::dom::DomTree;
use crate::interp::wrap63;
use crate::ir::{BinOp, BlockId, FuncId, Function, Inst, Module, Target, Term, UnOp, V, ValueDef};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptLevel {
    O0,
    O1,
    O2,
}

/// What each pass changed, for `--stats`.
#[derive(Default, Debug, Clone)]
pub struct Stats {
    pub folded: usize,
    pub branches_folded: usize,
    pub cse: usize,
    pub dce: usize,
    pub inlined: usize,
    pub devirtualized: usize,
    pub blocks_merged: usize,
    pub params_removed: usize,
    pub tail_calls: usize,
}

pub struct PassLog {
    pub name: &'static str,
    pub ir: String,
}

/// Runs the pipeline for an optimization level. With `verify`, checks the IR after every pass and panics
/// (with the pass name) if one breaks it. With `log`, records the IR after every pass.
pub fn optimize(m: &mut Module, level: OptLevel, verify: bool, mut log: Option<&mut Vec<PassLog>>) -> Stats {
    let mut stats = Stats::default();
    if level == OptLevel::O0 {
        cleanup(m);
        return stats;
    }
    let mut run = |m: &mut Module, name: &'static str, f: &dyn Fn(&mut Module, &mut Stats), stats: &mut Stats| {
        let before = if verify { Some(m.clone()) } else { None };
        f(m, stats);
        if let Some(before) = before
            && let Err(e) = crate::ir::verify(m)
        {
            let name_of_bad = e.split(':').next().unwrap_or("").trim_start_matches("in fn ").to_string();
            let prev =
                before.funcs.iter().find(|f| f.name == name_of_bad).map(|f| crate::ir::print_function(&before, f)).unwrap_or_default();
            panic!(
                "IR invalid after pass `{name}`: {e}
--- before the pass:
{prev}"
            );
        }
        if let Some(l) = log.as_deref_mut() {
            l.push(PassLog { name, ir: crate::ir::print_module(m) });
        }
    };
    let rounds = if level == OptLevel::O2 { 3 } else { 1 };
    run(m, "simplify-cfg", &|m, s| per_fn(m, s, simplify_cfg), &mut stats);
    run(m, "tail-calls", &tail_calls, &mut stats);
    for _ in 0..rounds {
        if level == OptLevel::O2 {
            run(m, "inline", &inline, &mut stats);
            run(m, "simplify-cfg", &|m, s| per_fn(m, s, simplify_cfg), &mut stats);
        }
        run(m, "sccp", &|m, s| per_fn(m, s, sccp), &mut stats);
        run(m, "instcombine", &instcombine, &mut stats);
        if level == OptLevel::O2 {
            run(m, "gvn", &|m, s| per_fn(m, s, gvn), &mut stats);
        }
        run(m, "dce", &|m, s| per_fn(m, s, dce), &mut stats);
        run(m, "simplify-cfg", &|m, s| per_fn(m, s, simplify_cfg), &mut stats);
    }
    cleanup(m);
    stats
}

fn per_fn(m: &mut Module, stats: &mut Stats, f: fn(&mut Function, &mut Stats)) {
    for func in &mut m.funcs {
        f(func, stats);
    }
}

/// Empties blocks that cannot be reached, so later stages never see them.
pub fn cleanup(m: &mut Module) {
    for f in &mut m.funcs {
        let live: HashSet<BlockId> = f.rpo().into_iter().collect();
        for (i, b) in f.blocks.iter_mut().enumerate() {
            if !live.contains(&(i as BlockId)) {
                b.insts.clear();
                b.params.clear();
                b.term = Term::Unreachable;
            }
        }
    }
}

fn use_counts(f: &Function) -> HashMap<V, usize> {
    let mut uses: HashMap<V, usize> = HashMap::new();
    for b in f.rpo() {
        let blk = &f.blocks[b as usize];
        for &v in &blk.insts {
            for o in f.inst(v).unwrap().operands() {
                *uses.entry(o).or_default() += 1;
            }
        }
        for o in blk.term.operands() {
            *uses.entry(o).or_default() += 1;
        }
    }
    uses
}

// ----------------------------------------------------------------------
// CFG simplification
// ----------------------------------------------------------------------

pub fn simplify_cfg(f: &mut Function, stats: &mut Stats) {
    for _ in 0..100 {
        let mut changed = false;
        changed |= remove_trivial_params(f, stats);
        changed |= fold_trivial_branches(f, stats);
        changed |= thread_jumps(f);
        changed |= merge_blocks(f, stats);
        if !changed {
            break;
        }
    }
}

/// Incoming edges of every block: (predecessor, index of the target in its terminator).
fn incoming(f: &Function) -> HashMap<BlockId, Vec<(BlockId, usize)>> {
    let mut inc: HashMap<BlockId, Vec<(BlockId, usize)>> = HashMap::new();
    for b in f.rpo() {
        for (i, t) in f.blocks[b as usize].term.targets().iter().enumerate() {
            inc.entry(t.block).or_default().push((b, i));
        }
    }
    inc
}

/// A parameter whose incoming values are all the same value (or the parameter itself) is that value.
fn remove_trivial_params(f: &mut Function, stats: &mut Stats) -> bool {
    let inc = incoming(f);
    let mut replace: HashMap<V, V> = HashMap::new();
    let mut removals: Vec<(BlockId, usize)> = Vec::new();
    for b in f.rpo() {
        if b == f.entry {
            continue;
        }
        let Some(edges) = inc.get(&b) else { continue };
        let params = f.blocks[b as usize].params.clone();
        for (i, &p) in params.iter().enumerate() {
            let mut same: Option<V> = None;
            let mut trivial = true;
            for &(pred, ti) in edges {
                let a = f.blocks[pred as usize].term.targets()[ti].args[i];
                if a == p || Some(a) == same {
                    continue;
                }
                if same.is_some() {
                    trivial = false;
                    break;
                }
                same = Some(a);
            }
            if trivial && let Some(s) = same {
                replace.insert(p, s);
                removals.push((b, i));
            }
        }
    }
    if removals.is_empty() {
        return false;
    }
    // Remove from the highest index down so indices stay valid.
    removals.sort_by_key(|r| std::cmp::Reverse(r.1));
    for &(b, i) in &removals {
        remove_param(f, b, i, &inc);
        stats.params_removed += 1;
    }
    f.replace_uses(&replace);
    true
}

fn remove_param(f: &mut Function, b: BlockId, i: usize, inc: &HashMap<BlockId, Vec<(BlockId, usize)>>) {
    f.blocks[b as usize].params.remove(i);
    for (j, &p) in f.blocks[b as usize].params.clone().iter().enumerate() {
        f.values[p as usize] = ValueDef::Param { block: b, index: j as u32 };
    }
    for &(pred, ti) in inc.get(&b).map(Vec::as_slice).unwrap_or(&[]) {
        f.blocks[pred as usize].term.targets_mut()[ti].args.remove(i);
    }
}

fn fold_trivial_branches(f: &mut Function, stats: &mut Stats) -> bool {
    let mut changed = false;
    for b in f.rpo() {
        let term = f.blocks[b as usize].term.clone();
        if let Term::Branch(c, t, e) = term {
            let new = if t == e {
                Some(t)
            } else if let Some(Inst::Const(n)) = f.inst(c) {
                Some(if *n != 0 { t } else { e })
            } else {
                None
            };
            if let Some(target) = new {
                f.blocks[b as usize].term = Term::Jump(target);
                stats.branches_folded += 1;
                changed = true;
            }
        }
    }
    changed
}

/// A block with no instructions that just jumps elsewhere can be skipped by its predecessors.
fn thread_jumps(f: &mut Function) -> bool {
    let mut changed = false;
    for b in f.rpo() {
        if b == f.entry {
            continue;
        }
        let blk = &f.blocks[b as usize];
        if !blk.insts.is_empty() {
            continue;
        }
        let Term::Jump(next) = blk.term.clone() else { continue };
        if next.block == b {
            continue;
        }
        let params = blk.params.clone();
        // The block's parameters must not be used anywhere but in its own jump: once its predecessors
        // skip it, nothing defines them any more.
        if !params.is_empty() {
            let uses = use_counts(f);
            let in_jump = |p: V| next.args.iter().filter(|&&a| a == p).count();
            if params.iter().any(|&p| uses.get(&p).copied().unwrap_or(0) != in_jump(p)) {
                continue;
            }
        }
        // The forwarded arguments may only mention this block's parameters (substituted per edge) or
        // values that dominate... values defined elsewhere are fine only if they are not from this block.
        for pred in f.rpo() {
            let n = f.blocks[pred as usize].term.targets().len();
            for ti in 0..n {
                let t = f.blocks[pred as usize].term.targets()[ti].clone();
                if t.block != b {
                    continue;
                }
                let args: Vec<V> = next.args.iter().map(|a| params.iter().position(|p| p == a).map_or(*a, |i| t.args[i])).collect();
                let tm = &mut f.blocks[pred as usize].term.targets_mut()[ti];
                tm.block = next.block;
                tm.args = args;
                changed = true;
            }
        }
    }
    changed
}

/// Merges a block into its predecessor when the predecessor jumps only to it and it has no other
/// predecessor.
fn merge_blocks(f: &mut Function, stats: &mut Stats) -> bool {
    let inc = incoming(f);
    let mut changed = false;
    for b in f.rpo() {
        let Term::Jump(t) = f.blocks[b as usize].term.clone() else { continue };
        let c = t.block;
        if c == b || c == f.entry {
            continue;
        }
        if inc.get(&c).map_or(0, Vec::len) != 1 {
            continue;
        }
        // Parameters of c become the jump arguments.
        let params = f.blocks[c as usize].params.clone();
        let map: HashMap<V, V> = params.iter().copied().zip(t.args.iter().copied()).collect();
        let insts = std::mem::take(&mut f.blocks[c as usize].insts);
        let term = std::mem::replace(&mut f.blocks[c as usize].term, Term::Unreachable);
        f.blocks[c as usize].params.clear();
        f.blocks[b as usize].insts.extend(insts);
        f.blocks[b as usize].term = term;
        f.replace_uses(&map);
        stats.blocks_merged += 1;
        changed = true;
        break; // incoming() is stale now
    }
    changed
}

// ----------------------------------------------------------------------
// Sparse conditional constant propagation
// ----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq)]
enum Lat {
    Top,
    C(i64),
    Bot,
}

fn meet(a: Lat, b: Lat) -> Lat {
    match (a, b) {
        (Lat::Top, x) | (x, Lat::Top) => x,
        (Lat::C(x), Lat::C(y)) if x == y => Lat::C(x),
        _ => Lat::Bot,
    }
}

/// Folds a binary operation on constants. None if it would trap (division by zero).
pub fn fold_binary(op: BinOp, x: i64, y: i64) -> Option<i64> {
    Some(match op {
        BinOp::Add => wrap63(x.wrapping_add(y)),
        BinOp::Sub => wrap63(x.wrapping_sub(y)),
        BinOp::Mul => wrap63(x.wrapping_mul(y)),
        BinOp::Div => {
            if y == 0 {
                return None;
            }
            wrap63(x.wrapping_div(y))
        }
        BinOp::Mod => {
            if y == 0 {
                return None;
            }
            wrap63(x.wrapping_rem(y))
        }
        BinOp::Lt => i64::from(x < y),
        BinOp::Le => i64::from(x <= y),
        BinOp::Gt => i64::from(x > y),
        BinOp::Ge => i64::from(x >= y),
        BinOp::Eq => i64::from(x == y),
        BinOp::Ne => i64::from(x != y),
    })
}

pub fn sccp(f: &mut Function, stats: &mut Stats) {
    let n = f.values.len();
    let mut lat = vec![Lat::Top; n];
    let mut exec: HashSet<BlockId> = HashSet::new();
    let mut exec_edges: HashSet<(BlockId, usize)> = HashSet::new();
    exec.insert(f.entry);
    for &p in f.params() {
        lat[p as usize] = Lat::Bot;
    }
    let rpo = f.rpo();
    let preds = incoming(f);
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 1000 {
        changed = false;
        rounds += 1;
        for &b in &rpo {
            if !exec.contains(&b) {
                continue;
            }
            let blk = &f.blocks[b as usize];
            if b != f.entry {
                for (i, &p) in blk.params.iter().enumerate() {
                    let mut v = Lat::Top;
                    for &(pred, ti) in preds.get(&b).map(Vec::as_slice).unwrap_or(&[]) {
                        if exec_edges.contains(&(pred, ti)) {
                            let a = f.blocks[pred as usize].term.targets()[ti].args[i];
                            v = meet(v, lat[a as usize]);
                        }
                    }
                    if v != lat[p as usize] {
                        lat[p as usize] = meet(lat[p as usize], v);
                        changed = true;
                    }
                }
            }
            for &v in &blk.insts {
                let new = eval(f, f.inst(v).unwrap(), &lat);
                let merged = if lat[v as usize] == Lat::Top { new } else { meet(lat[v as usize], new) };
                if merged != lat[v as usize] {
                    lat[v as usize] = merged;
                    changed = true;
                }
            }
            let mark = |ti: usize, to: BlockId, exec: &mut HashSet<BlockId>, edges: &mut HashSet<(BlockId, usize)>| {
                if edges.insert((b, ti)) {
                    exec.insert(to);
                    true
                } else {
                    false
                }
            };
            match &blk.term {
                Term::Jump(t) => changed |= mark(0, t.block, &mut exec, &mut exec_edges),
                Term::Branch(c, t, e) => match lat[*c as usize] {
                    Lat::Top => {}
                    Lat::C(k) => {
                        if k != 0 {
                            changed |= mark(0, t.block, &mut exec, &mut exec_edges);
                        } else {
                            changed |= mark(1, e.block, &mut exec, &mut exec_edges);
                        }
                    }
                    Lat::Bot => {
                        changed |= mark(0, t.block, &mut exec, &mut exec_edges);
                        changed |= mark(1, e.block, &mut exec, &mut exec_edges);
                    }
                },
                _ => {}
            }
        }
    }

    // Rewrite: constant instructions become `const`, constant parameters are replaced by a constant in the
    // entry block, and branches with a known condition become jumps.
    let mut param_consts: HashMap<V, V> = HashMap::new();
    let mut new_consts: Vec<V> = Vec::new();
    for &b in &rpo {
        if !exec.contains(&b) {
            continue;
        }
        let blk = f.blocks[b as usize].clone();
        for &v in &blk.insts {
            if let Lat::C(k) = lat[v as usize] {
                let inst = f.inst(v).unwrap();
                if !matches!(inst, Inst::Const(_)) && inst.is_pure() || matches!(inst, Inst::Binary(BinOp::Div | BinOp::Mod, ..)) {
                    *f.inst_mut(v).unwrap() = Inst::Const(k);
                    stats.folded += 1;
                }
            }
        }
        if b != f.entry {
            for &p in &blk.params {
                if let Lat::C(k) = lat[p as usize] {
                    let c = f.make(Inst::Const(k));
                    new_consts.push(c);
                    param_consts.insert(p, c);
                    stats.folded += 1;
                }
            }
        }
        if let Term::Branch(c, t, e) = &blk.term
            && let Lat::C(k) = lat[*c as usize]
        {
            f.blocks[b as usize].term = Term::Jump(if k != 0 { t.clone() } else { e.clone() });
            stats.branches_folded += 1;
        }
    }
    if !new_consts.is_empty() {
        let entry = f.entry as usize;
        let mut insts = new_consts;
        insts.append(&mut f.blocks[entry].insts);
        f.blocks[entry].insts = insts;
        f.replace_uses(&param_consts);
    }
}

fn eval(f: &Function, inst: &Inst, lat: &[Lat]) -> Lat {
    let l = |v: &V| lat[*v as usize];
    match inst {
        Inst::Const(k) => Lat::C(*k),
        Inst::Unary(op, a) => match l(a) {
            Lat::C(x) => Lat::C(match op {
                UnOp::Neg => wrap63(x.wrapping_neg()),
                UnOp::Not => i64::from(x == 0),
            }),
            other => other,
        },
        Inst::Binary(op, a, b) => match (l(a), l(b)) {
            (Lat::C(x), Lat::C(y)) => fold_binary(*op, x, y).map_or(Lat::Bot, Lat::C),
            (Lat::Top, _) | (_, Lat::Top) => Lat::Top,
            _ => Lat::Bot,
        },
        Inst::Tag(a) => match f.inst(*a) {
            Some(Inst::Alloc { tag, .. }) => Lat::C(i64::from(*tag)),
            _ => Lat::Bot,
        },
        _ => Lat::Bot,
    }
}

// ----------------------------------------------------------------------
// Instruction combining
// ----------------------------------------------------------------------

pub fn instcombine(m: &mut Module, stats: &mut Stats) {
    let ctor_tags: Vec<u32> = m.ctor0.iter().map(|c| c.tag).collect();
    let takes_env: Vec<bool> = m.funcs.iter().map(|f| f.takes_env).collect();
    for f in &mut m.funcs {
        let mut replace: HashMap<V, V> = HashMap::new();
        for b in f.rpo() {
            for v in f.blocks[b as usize].insts.clone() {
                let inst = f.inst(v).unwrap().clone();
                let def = |x: V| f.inst(x).cloned();
                let konst = |x: V| if let Some(Inst::Const(k)) = f.inst(x) { Some(*k) } else { None };
                let simplified: Option<Result<V, Inst>> = match &inst {
                    Inst::Binary(op, a, b) => {
                        let (ka, kb) = (konst(*a), konst(*b));
                        match (op, ka, kb) {
                            (_, Some(x), Some(y)) => fold_binary(*op, x, y).map(|k| Err(Inst::Const(k))),
                            (BinOp::Add, Some(0), _) => Some(Ok(*b)),
                            (BinOp::Add | BinOp::Sub, _, Some(0)) => Some(Ok(*a)),
                            (BinOp::Mul, Some(1), _) => Some(Ok(*b)),
                            (BinOp::Mul | BinOp::Div, _, Some(1)) => Some(Ok(*a)),
                            (BinOp::Mul, Some(0), _) | (BinOp::Mul, _, Some(0)) => Some(Err(Inst::Const(0))),
                            (BinOp::Sub, _, _) if a == b => Some(Err(Inst::Const(0))),
                            (BinOp::Eq | BinOp::Le | BinOp::Ge, _, _) if a == b => Some(Err(Inst::Const(1))),
                            (BinOp::Ne | BinOp::Lt | BinOp::Gt, _, _) if a == b => Some(Err(Inst::Const(0))),
                            _ => None,
                        }
                    }
                    Inst::Unary(UnOp::Not, a) => match def(*a) {
                        Some(Inst::Unary(UnOp::Not, x)) => Some(Ok(x)),
                        Some(Inst::Binary(op @ (BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne), x, y)) => {
                            let inv = match op {
                                BinOp::Lt => BinOp::Ge,
                                BinOp::Le => BinOp::Gt,
                                BinOp::Gt => BinOp::Le,
                                BinOp::Ge => BinOp::Lt,
                                BinOp::Eq => BinOp::Ne,
                                _ => BinOp::Eq,
                            };
                            Some(Err(Inst::Binary(inv, x, y)))
                        }
                        _ => None,
                    },
                    Inst::Unary(UnOp::Neg, a) => match def(*a) {
                        Some(Inst::Unary(UnOp::Neg, x)) => Some(Ok(x)),
                        _ => None,
                    },
                    // Loads from objects built in this function.
                    Inst::Field(o, i) => match def(*o) {
                        Some(Inst::Alloc { fields, .. }) => fields.get(*i as usize).map(|&x| Ok(x)),
                        Some(Inst::MakeClosure { captures, .. }) if *i >= 1 => captures.get(*i as usize - 1).map(|&x| Ok(x)),
                        _ => None,
                    },
                    Inst::Tag(o) => match def(*o) {
                        Some(Inst::Alloc { tag, .. }) => Some(Err(Inst::Const(i64::from(tag)))),
                        Some(Inst::Ctor0(c)) => Some(Err(Inst::Const(i64::from(ctor_tags[c as usize])))),
                        _ => None,
                    },
                    // A call through a closure whose function is known becomes a direct call.
                    Inst::CallClosure(c, args) => match def(*c) {
                        Some(Inst::FuncRef(g)) | Some(Inst::MakeClosure { func: g, .. }) if takes_env[g as usize] => {
                            stats.devirtualized += 1;
                            Some(Err(Inst::Call(g, std::iter::once(*c).chain(args.iter().copied()).collect())))
                        }
                        _ => None,
                    },
                    _ => None,
                };
                match simplified {
                    Some(Ok(x)) => {
                        replace.insert(v, x);
                        stats.folded += 1;
                    }
                    Some(Err(new)) => {
                        *f.inst_mut(v).unwrap() = new;
                        stats.folded += 1;
                    }
                    None => {}
                }
            }
        }
        f.replace_uses(&replace);
        // `br !x, t, e` is `br x, e, t` (the negation then usually dies).
        for b in f.rpo() {
            let Term::Branch(c, _, _) = &f.blocks[b as usize].term else { continue };
            if let Some(&Inst::Unary(UnOp::Not, x)) = f.inst(*c)
                && let Term::Branch(_, t, e) = std::mem::replace(&mut f.blocks[b as usize].term, Term::Unreachable)
            {
                f.blocks[b as usize].term = Term::Branch(x, e, t);
                stats.folded += 1;
            }
        }
    }
}

// ----------------------------------------------------------------------
// Global value numbering
// ----------------------------------------------------------------------

pub fn gvn(f: &mut Function, stats: &mut Stats) {
    let dom = DomTree::compute(f);
    let mut replace: HashMap<V, V> = HashMap::new();
    // Scoped table: walk the dominator tree, remembering which entries each block added.
    let mut table: HashMap<Inst, V> = HashMap::new();
    enum Step {
        Enter(BlockId),
        Leave(Vec<Inst>),
    }
    let mut stack = vec![Step::Enter(dom.entry)];
    while let Some(step) = stack.pop() {
        match step {
            Step::Leave(keys) => {
                for k in keys {
                    table.remove(&k);
                }
            }
            Step::Enter(b) => {
                let mut added = Vec::new();
                for v in f.blocks[b as usize].insts.clone() {
                    let mut inst = f.inst(v).unwrap().clone();
                    inst.map_operands(&mut |x| *replace.get(&x).unwrap_or(&x));
                    if !inst.is_cse_candidate() {
                        continue;
                    }
                    let key = canonical(inst);
                    match table.get(&key) {
                        Some(&prev) => {
                            replace.insert(v, prev);
                            stats.cse += 1;
                        }
                        None => {
                            table.insert(key.clone(), v);
                            added.push(key);
                        }
                    }
                }
                stack.push(Step::Leave(added));
                if let Some(cs) = dom.children.get(&b) {
                    for &c in cs.iter().rev() {
                        stack.push(Step::Enter(c));
                    }
                }
            }
        }
    }
    f.replace_uses(&replace);
}

/// Orders the operands of commutative operations, so `a + b` and `b + a` are the same expression.
fn canonical(i: Inst) -> Inst {
    match i {
        Inst::Binary(op @ (BinOp::Add | BinOp::Mul | BinOp::Eq | BinOp::Ne), a, b) if a > b => Inst::Binary(op, b, a),
        other => other,
    }
}

// ----------------------------------------------------------------------
// Dead code elimination
// ----------------------------------------------------------------------

pub fn dce(f: &mut Function, stats: &mut Stats) {
    let rpo = f.rpo();
    let mut live: HashSet<V> = HashSet::new();
    let mut work: Vec<V> = Vec::new();
    for &b in &rpo {
        let blk = &f.blocks[b as usize];
        for &v in &blk.insts {
            if !f.inst(v).unwrap().is_pure() {
                work.push(v);
            }
        }
        // Branch conditions and return values are always needed; jump arguments only if their parameter is.
        match &blk.term {
            Term::Branch(c, _, _) => work.push(*c),
            Term::Return(v) => work.push(*v),
            _ => {}
        }
    }
    let inc = incoming(f);
    while let Some(v) = work.pop() {
        if !live.insert(v) {
            continue;
        }
        match &f.values[v as usize] {
            ValueDef::Inst(i) => work.extend(i.operands()),
            ValueDef::Param { block, index } => {
                if *block == f.entry {
                    continue;
                }
                for &(pred, ti) in inc.get(block).map(Vec::as_slice).unwrap_or(&[]) {
                    work.push(f.blocks[pred as usize].term.targets()[ti].args[*index as usize]);
                }
            }
        }
    }
    for &b in &rpo {
        let before = f.blocks[b as usize].insts.len();
        let keep: Vec<V> = f.blocks[b as usize].insts.iter().copied().filter(|v| live.contains(v)).collect();
        stats.dce += before - keep.len();
        f.blocks[b as usize].insts = keep;
    }
    // Dead block parameters (never for the entry block, whose parameters are the function's).
    for &b in rpo.iter().rev() {
        if b == f.entry {
            continue;
        }
        let params = f.blocks[b as usize].params.clone();
        for i in (0..params.len()).rev() {
            if !live.contains(&params[i]) {
                remove_param(f, b, i, &inc);
                stats.params_removed += 1;
            }
        }
    }
}

// ----------------------------------------------------------------------
// Inlining
// ----------------------------------------------------------------------

const INLINE_SIZE: usize = 30;
const MAX_CALLER_SIZE: usize = 2000;

pub fn inline(m: &mut Module, stats: &mut Stats) {
    // Never inline a function into one it can reach (that would not terminate, or would unroll recursion).
    let recursive = recursive_functions(m);
    let sizes: Vec<usize> = m.funcs.iter().map(Function::size).collect();
    for caller in 0..m.funcs.len() {
        let mut budget = MAX_CALLER_SIZE.saturating_sub(sizes[caller]);
        loop {
            let site = find_call_site(&m.funcs[caller], |callee| {
                callee as usize != caller && !recursive.contains(&callee) && sizes[callee as usize] <= INLINE_SIZE
            });
            let Some((block, pos, callee)) = site else { break };
            if sizes[callee as usize] > budget {
                break;
            }
            budget -= sizes[callee as usize];
            let callee_fn = m.funcs[callee as usize].clone();
            inline_at(&mut m.funcs[caller], block, pos, &callee_fn);
            stats.inlined += 1;
        }
    }
}

fn find_call_site(f: &Function, ok: impl Fn(FuncId) -> bool) -> Option<(BlockId, usize, FuncId)> {
    for b in f.rpo() {
        for (i, &v) in f.blocks[b as usize].insts.iter().enumerate() {
            if let Some(Inst::Call(g, _)) = f.inst(v)
                && ok(*g)
            {
                return Some((b, i, *g));
            }
        }
    }
    None
}

/// Functions on a cycle of the call graph.
fn recursive_functions(m: &Module) -> HashSet<FuncId> {
    let calls: Vec<HashSet<FuncId>> = m
        .funcs
        .iter()
        .map(|f| {
            let mut s = HashSet::new();
            for b in f.rpo() {
                for &v in &f.blocks[b as usize].insts {
                    match f.inst(v) {
                        Some(Inst::Call(g, _)) => {
                            s.insert(*g);
                        }
                        // Calls through closures may go anywhere a closure is made for.
                        Some(Inst::FuncRef(g)) | Some(Inst::MakeClosure { func: g, .. }) => {
                            s.insert(*g);
                        }
                        _ => {}
                    }
                }
            }
            s
        })
        .collect();
    let mut rec = HashSet::new();
    for start in 0..m.funcs.len() as FuncId {
        // Is `start` reachable from its own callees?
        let mut seen = HashSet::new();
        let mut stack: Vec<FuncId> = calls[start as usize].iter().copied().collect();
        while let Some(g) = stack.pop() {
            if g == start {
                rec.insert(start);
                break;
            }
            if seen.insert(g) {
                stack.extend(calls[g as usize].iter().copied());
            }
        }
    }
    rec
}

/// Replaces the call at `insts[pos]` of `block` with a copy of `callee`'s body.
fn inline_at(f: &mut Function, block: BlockId, pos: usize, callee: &Function) {
    let call_v = f.blocks[block as usize].insts[pos];
    let Some(Inst::Call(_, args)) = f.inst(call_v).cloned() else { unreachable!() };

    // Split: the instructions after the call move to a continuation block that receives the result.
    let cont = f.new_block();
    let result = f.add_param(cont);
    let after: Vec<V> = f.blocks[block as usize].insts.split_off(pos + 1);
    f.blocks[block as usize].insts.pop(); // the call itself
    let old_term = std::mem::replace(&mut f.blocks[block as usize].term, Term::Unreachable);
    f.blocks[cont as usize].insts = after;
    f.blocks[cont as usize].term = old_term;

    // Copy the callee's blocks and values.
    let mut vmap: HashMap<V, V> = HashMap::new();
    let mut bmap: HashMap<BlockId, BlockId> = HashMap::new();
    let order = callee.rpo();
    for &cb in &order {
        bmap.insert(cb, f.new_block());
    }
    for &cb in &order {
        let nb = bmap[&cb];
        for &p in &callee.blocks[cb as usize].params {
            let np = f.add_param(nb);
            vmap.insert(p, np);
        }
    }
    for &cb in &order {
        for &v in &callee.blocks[cb as usize].insts {
            let nv = f.make(Inst::Const(0));
            vmap.insert(v, nv);
        }
    }
    for &cb in &order {
        let nb = bmap[&cb];
        for &v in &callee.blocks[cb as usize].insts {
            let mut inst = callee.inst(v).unwrap().clone();
            inst.map_operands(&mut |x| vmap[&x]);
            let nv = vmap[&v];
            *f.inst_mut(nv).unwrap() = inst;
            f.blocks[nb as usize].insts.push(nv);
        }
        let term = match &callee.blocks[cb as usize].term {
            Term::Jump(t) => Term::Jump(Target { block: bmap[&t.block], args: t.args.iter().map(|a| vmap[a]).collect() }),
            Term::Branch(c, t, e) => Term::Branch(
                vmap[c],
                Target { block: bmap[&t.block], args: t.args.iter().map(|a| vmap[a]).collect() },
                Target { block: bmap[&e.block], args: e.args.iter().map(|a| vmap[a]).collect() },
            ),
            Term::Return(v) => Term::Jump(Target { block: cont, args: vec![vmap[v]] }),
            Term::Panic(s) => Term::Panic(*s),
            Term::Unreachable => Term::Unreachable,
        };
        f.blocks[nb as usize].term = term;
    }
    f.blocks[block as usize].term = Term::Jump(Target { block: bmap[&callee.entry], args });
    let mut map = HashMap::new();
    map.insert(call_v, result);
    f.replace_uses(&map);
}

// ----------------------------------------------------------------------
// Tail calls
// ----------------------------------------------------------------------

/// Turns self-recursive calls in tail position into jumps back to the top of the function, so that
/// tail-recursive functions run as loops (in constant stack space).
///
/// A call is in tail position when its block returns its result directly, or jumps to a block that does
/// nothing but return the value it receives (the join block of an `if` or `match`).
pub fn tail_calls(m: &mut Module, stats: &mut Stats) {
    for fid in 0..m.funcs.len() {
        let f = &mut m.funcs[fid];
        let sites = tail_call_sites(f, fid as FuncId);
        if sites.is_empty() {
            continue;
        }
        // The old entry becomes the loop header: a new entry jumps to it with the function's parameters,
        // and every tail call jumps to it with its arguments.
        let header = f.entry;
        let new_entry = f.new_block();
        let n = f.blocks[header as usize].params.len();
        let params: Vec<V> = (0..n).map(|_| f.add_param(new_entry)).collect();
        f.blocks[new_entry as usize].term = Term::Jump(Target { block: header, args: params });
        f.entry = new_entry;
        for (b, call) in sites {
            let Some(Inst::Call(_, args)) = f.inst(call).cloned() else { continue };
            f.blocks[b as usize].insts.retain(|&v| v != call);
            f.blocks[b as usize].term = Term::Jump(Target { block: header, args });
            stats.tail_calls += 1;
        }
    }
}

fn tail_call_sites(f: &Function, me: FuncId) -> Vec<(BlockId, V)> {
    let uses = use_counts(f);
    let returns_param = |b: BlockId| -> bool {
        let blk = &f.blocks[b as usize];
        blk.insts.is_empty() && blk.params.len() == 1 && matches!(blk.term, Term::Return(v) if v == blk.params[0])
    };
    let mut sites = Vec::new();
    for b in f.rpo() {
        let blk = &f.blocks[b as usize];
        let Some(&last) = blk.insts.last() else { continue };
        if !matches!(f.inst(last), Some(Inst::Call(g, _)) if *g == me) || uses.get(&last).copied().unwrap_or(0) != 1 {
            continue;
        }
        let tail = match &blk.term {
            Term::Return(v) => *v == last,
            Term::Jump(t) => t.args == [last] && returns_param(t.block),
            _ => false,
        };
        if tail {
            sites.push((b, last));
        }
    }
    sites
}
