//! Register allocation by linear scan (Poletto and Sarkar, "Linear Scan Register Allocation", TOPLAS 1999).
//!
//! Every virtual register gets one live interval, from its first definition to its last use in the block
//! layout order (extended over every block it is live through, which covers loops). Intervals are visited
//! by start position; each gets a free register, or, when none is left, the interval among the current
//! ones that ends last is spilled to a stack slot.
//!
//! Calls clobber the caller-saved registers, so an interval that is live across a call may only get a
//! callee-saved register (or a stack slot). That also keeps every live value where the garbage collector,
//! which scans the stack and the callee-saved registers, can see it.

use std::collections::{HashMap, HashSet};

use super::x86::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assign {
    Reg(Reg),
    Slot(u32),
}

pub struct Allocation {
    pub assign: Vec<Option<Assign>>,
    pub slots: u32,
    pub callee_saved_used: Vec<Reg>,
}

#[derive(Clone, Debug)]
struct Interval {
    vreg: u32,
    start: u32,
    end: u32,
    crosses_call: bool,
}

pub fn allocate(f: &MFunction, abi: Abi) -> Allocation {
    let n = f.vregs as usize;
    // Positions: instruction i (in layout order) reads its operands at 2i and writes its results at 2i+1.
    let mut block_start = Vec::with_capacity(f.blocks.len());
    let mut block_end = Vec::with_capacity(f.blocks.len());
    let mut pos = 0u32;
    let mut calls: Vec<u32> = Vec::new();
    for b in &f.blocks {
        block_start.push(pos);
        for i in &b.insts {
            if i.is_call() {
                calls.push(pos);
            }
            pos += 2;
        }
        block_end.push(pos.saturating_sub(1).max(block_start.last().copied().unwrap_or(0)));
    }

    // Liveness (backward data flow over the block graph).
    let nb = f.blocks.len();
    let succs: Vec<Vec<usize>> = (0..nb).map(|b| f.successors(b)).collect();
    let mut gen_: Vec<HashSet<u32>> = vec![HashSet::new(); nb];
    let mut kill: Vec<HashSet<u32>> = vec![HashSet::new(); nb];
    for (b, blk) in f.blocks.iter().enumerate() {
        for i in &blk.insts {
            let (uses, defs) = i.uses_defs();
            for u in uses {
                if !kill[b].contains(&u) {
                    gen_[b].insert(u);
                }
            }
            for d in defs {
                kill[b].insert(d);
            }
        }
    }
    let mut live_in: Vec<HashSet<u32>> = vec![HashSet::new(); nb];
    let mut live_out: Vec<HashSet<u32>> = vec![HashSet::new(); nb];
    let mut changed = true;
    while changed {
        changed = false;
        for b in (0..nb).rev() {
            let mut out: HashSet<u32> = HashSet::new();
            for &s in &succs[b] {
                out.extend(live_in[s].iter().copied());
            }
            let mut inn: HashSet<u32> = gen_[b].clone();
            for v in &out {
                if !kill[b].contains(v) {
                    inn.insert(*v);
                }
            }
            if inn != live_in[b] || out != live_out[b] {
                live_in[b] = inn;
                live_out[b] = out;
                changed = true;
            }
        }
    }

    // Intervals.
    let mut start = vec![u32::MAX; n];
    let mut end = vec![0u32; n];
    let touch = |v: u32, p: u32, start: &mut Vec<u32>, end: &mut Vec<u32>| {
        let v = v as usize;
        start[v] = start[v].min(p);
        end[v] = end[v].max(p);
    };
    let mut p = 0u32;
    for (b, blk) in f.blocks.iter().enumerate() {
        for &v in &live_in[b] {
            touch(v, block_start[b], &mut start, &mut end);
        }
        for &v in &live_out[b] {
            touch(v, block_end[b], &mut start, &mut end);
        }
        for i in &blk.insts {
            let (uses, defs) = i.uses_defs();
            for u in uses {
                touch(u, p, &mut start, &mut end);
            }
            for d in defs {
                touch(d, p + 1, &mut start, &mut end);
            }
            p += 2;
        }
    }
    let mut intervals: Vec<Interval> = (0..n as u32)
        .filter(|&v| start[v as usize] != u32::MAX)
        .map(|v| {
            let (s, e) = (start[v as usize], end[v as usize]);
            let crosses_call = calls.iter().any(|&c| s < c && e > c);
            Interval { vreg: v, start: s, end: e, crosses_call }
        })
        .collect();
    intervals.sort_by_key(|i| (i.start, i.end));

    // Linear scan.
    let regs = abi.allocatable();
    let mut assign: Vec<Option<Assign>> = vec![None; n];
    let mut active: Vec<(Interval, Reg)> = Vec::new();
    let mut free: Vec<Reg> = regs.clone();
    let mut slots = 0u32;
    let mut used: HashSet<Reg> = HashSet::new();
    for cur in intervals {
        // Expire intervals that ended before this one starts.
        active.retain(|(iv, r)| {
            if iv.end < cur.start {
                free.push(*r);
                false
            } else {
                true
            }
        });
        let ok = |r: &Reg| !cur.crosses_call || abi.is_callee_saved(*r);
        // Prefer caller-saved registers for short intervals, to keep callee-saved ones for values that
        // live across calls.
        let choice = regs.iter().copied().find(|r| free.contains(r) && ok(r));
        match choice {
            Some(r) => {
                free.retain(|x| *x != r);
                used.insert(r);
                assign[cur.vreg as usize] = Some(Assign::Reg(r));
                active.push((cur, r));
            }
            None => {
                // Spill whichever compatible interval ends last.
                let victim = active
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, r))| ok(r))
                    .max_by_key(|(_, (iv, _))| iv.end)
                    .map(|(i, (iv, r))| (i, iv.end, *r));
                match victim {
                    Some((i, vend, r)) if vend > cur.end => {
                        let (old, _) = active.remove(i);
                        assign[old.vreg as usize] = Some(Assign::Slot(slots));
                        slots += 1;
                        assign[cur.vreg as usize] = Some(Assign::Reg(r));
                        active.push((cur, r));
                    }
                    _ => {
                        assign[cur.vreg as usize] = Some(Assign::Slot(slots));
                        slots += 1;
                    }
                }
            }
        }
    }
    let mut callee_saved_used: Vec<Reg> = used.into_iter().filter(|r| abi.is_callee_saved(*r)).collect();
    callee_saved_used.sort();
    let _ = HashMap::<u32, u32>::new();
    Allocation { assign, slots, callee_saved_used }
}
