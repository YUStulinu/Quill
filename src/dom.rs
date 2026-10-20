//! Dominator trees, by the Cooper–Harvey–Kennedy iterative algorithm ("A Simple, Fast Dominance
//! Algorithm", 2001).

use std::collections::HashMap;

use crate::ir::{BlockId, Function};

pub struct DomTree {
    /// Immediate dominator of each reachable block (the entry dominates itself).
    pub idom: HashMap<BlockId, BlockId>,
    /// Position of each block in reverse post-order.
    order: HashMap<BlockId, usize>,
    pub children: HashMap<BlockId, Vec<BlockId>>,
    pub entry: BlockId,
}

impl DomTree {
    pub fn compute(f: &Function) -> DomTree {
        let rpo = f.rpo();
        let order: HashMap<BlockId, usize> = rpo.iter().enumerate().map(|(i, &b)| (b, i)).collect();
        let preds = f.predecessors();
        let mut idom: HashMap<BlockId, BlockId> = HashMap::new();
        idom.insert(f.entry, f.entry);
        let intersect = |idom: &HashMap<BlockId, BlockId>, mut a: BlockId, mut b: BlockId| {
            while a != b {
                while order[&a] > order[&b] {
                    a = idom[&a];
                }
                while order[&b] > order[&a] {
                    b = idom[&b];
                }
            }
            a
        };
        let mut changed = true;
        while changed {
            changed = false;
            for &b in rpo.iter().skip(1) {
                let ps = preds.get(&b).cloned().unwrap_or_default();
                let mut new: Option<BlockId> = None;
                for p in ps {
                    if !idom.contains_key(&p) {
                        continue;
                    }
                    new = Some(match new {
                        None => p,
                        Some(n) => intersect(&idom, p, n),
                    });
                }
                if let Some(n) = new
                    && idom.get(&b) != Some(&n)
                {
                    idom.insert(b, n);
                    changed = true;
                }
            }
        }
        let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
        for &b in &rpo {
            if b != f.entry
                && let Some(&d) = idom.get(&b)
            {
                children.entry(d).or_default().push(b);
            }
        }
        DomTree { idom, order, children, entry: f.entry }
    }

    /// Does `a` dominate `b`? (Every block dominates itself.)
    pub fn dominates(&self, a: BlockId, b: BlockId) -> bool {
        if !self.order.contains_key(&a) || !self.order.contains_key(&b) {
            return false;
        }
        let mut x = b;
        loop {
            if x == a {
                return true;
            }
            if x == self.entry {
                return false;
            }
            x = self.idom[&x];
        }
    }

    /// Blocks in dominator-tree pre-order.
    pub fn preorder(&self) -> Vec<BlockId> {
        let mut out = Vec::new();
        let mut stack = vec![self.entry];
        while let Some(b) = stack.pop() {
            out.push(b);
            if let Some(cs) = self.children.get(&b) {
                for &c in cs.iter().rev() {
                    stack.push(c);
                }
            }
        }
        out
    }
}
