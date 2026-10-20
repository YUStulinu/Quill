//! Exhaustiveness and redundancy of `match`, by Maranget's usefulness algorithm ("Warnings for pattern
//! matching", JFP 2007). A pattern vector is *useful* with respect to a matrix of earlier patterns if some
//! value matches it and none of the rows. A match is exhaustive if the wildcard is not useful after all
//! its (unguarded) arms; an arm is unreachable if it is not useful after the arms before it. When the
//! wildcard is useful, the algorithm also builds a witness: an example of a value nobody handles.

use crate::ast::{Arm, PatKind, Pattern};
use crate::typeck::Typed;
use crate::types::{Ty, subst_gen};

#[derive(Clone, Debug, PartialEq)]
enum Key {
    Adt(u32),
    Bool(bool),
    /// Tuples (including unit) have a single constructor.
    Tuple(usize),
    Int(i64),
    Str(String),
}

#[derive(Clone, Debug)]
enum P {
    Wild,
    Ctor(Key, Vec<P>),
}

pub struct MatchReport {
    /// An example of a value no arm matches, if there is one.
    pub missing: Option<String>,
    /// Indices of arms that can never be chosen.
    pub unreachable: Vec<usize>,
}

pub fn check_match(typed: &Typed, scrut_ty: &Ty, arms: &[Arm]) -> MatchReport {
    let ctx = Ctx { typed };
    let mut rows: Vec<Vec<P>> = Vec::new();
    let mut unreachable = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let p = ctx.lower(&arm.pat);
        if ctx.useful(&rows, std::slice::from_ref(&p), std::slice::from_ref(scrut_ty)).is_none() {
            unreachable.push(i);
        }
        if arm.guard.is_none() {
            rows.push(vec![p]);
        }
    }
    let missing = ctx.useful(&rows, &[P::Wild], std::slice::from_ref(scrut_ty)).map(|w| ctx.show(&w[0], scrut_ty));
    MatchReport { missing, unreachable }
}

struct Ctx<'a> {
    typed: &'a Typed,
}

impl<'a> Ctx<'a> {
    fn lower(&self, p: &Pattern) -> P {
        match &p.kind {
            PatKind::Wildcard | PatKind::Bind(_) => P::Wild,
            PatKind::Int(n) => P::Ctor(Key::Int(*n), vec![]),
            PatKind::Bool(b) => P::Ctor(Key::Bool(*b), vec![]),
            PatKind::Str(s) => P::Ctor(Key::Str(s.clone()), vec![]),
            PatKind::Unit => P::Ctor(Key::Tuple(0), vec![]),
            PatKind::Tuple(ps) => P::Ctor(Key::Tuple(ps.len()), ps.iter().map(|q| self.lower(q)).collect()),
            PatKind::Ctor(name, ps) => match self.typed.ctor_index.get(name) {
                Some(&(_, tag)) => P::Ctor(Key::Adt(tag), ps.iter().map(|q| self.lower(q)).collect()),
                None => P::Wild,
            },
        }
    }

    /// Every constructor of a type, when there are finitely many.
    fn all_ctors(&self, t: &Ty) -> Option<Vec<Key>> {
        match t {
            Ty::Con(n, _) if &**n == "Bool" => Some(vec![Key::Bool(false), Key::Bool(true)]),
            Ty::Con(n, _) => {
                let &i = self.typed.adt_index.get(&**n)?;
                Some(self.typed.adts[i].ctors.iter().map(|c| Key::Adt(c.tag)).collect())
            }
            Ty::Tuple(es) => Some(vec![Key::Tuple(es.len())]),
            _ => None,
        }
    }

    /// The types of a constructor's fields, given the type of the value.
    fn field_tys(&self, k: &Key, t: &Ty) -> Vec<Ty> {
        match (k, t) {
            (Key::Tuple(_), Ty::Tuple(es)) => es.clone(),
            (Key::Adt(tag), Ty::Con(n, args)) => match self.typed.adt_index.get(&**n) {
                Some(&i) => self.typed.adts[i].ctors[*tag as usize].fields.iter().map(|f| subst_gen(f, args)).collect(),
                None => vec![],
            },
            (Key::Tuple(n), _) => vec![Ty::Var(u32::MAX); *n],
            (Key::Adt(_), _) => vec![],
            _ => vec![],
        }
    }

    fn arity(&self, k: &Key, t: &Ty) -> usize {
        match k {
            Key::Tuple(n) => *n,
            Key::Adt(_) => self.field_tys(k, t).len(),
            _ => 0,
        }
    }

    /// Rows whose first pattern can match constructor `k`, with that column replaced by its fields.
    fn specialize(&self, rows: &[Vec<P>], k: &Key, arity: usize) -> Vec<Vec<P>> {
        rows.iter()
            .filter_map(|row| match &row[0] {
                P::Wild => Some(std::iter::repeat_n(P::Wild, arity).chain(row[1..].iter().cloned()).collect()),
                P::Ctor(k2, args) if k2 == k => Some(args.iter().cloned().chain(row[1..].iter().cloned()).collect()),
                P::Ctor(..) => None,
            })
            .collect()
    }

    fn useful(&self, rows: &[Vec<P>], q: &[P], tys: &[Ty]) -> Option<Vec<P>> {
        if q.is_empty() {
            return if rows.is_empty() { Some(vec![]) } else { None };
        }
        let t0 = &tys[0];
        match &q[0] {
            P::Ctor(k, args) => {
                let arity = args.len();
                let rows2 = self.specialize(rows, k, arity);
                let q2: Vec<P> = args.iter().cloned().chain(q[1..].iter().cloned()).collect();
                let tys2: Vec<Ty> = pad(self.field_tys(k, t0), arity).into_iter().chain(tys[1..].iter().cloned()).collect();
                self.useful(&rows2, &q2, &tys2).map(|w| rebuild(k.clone(), arity, w))
            }
            P::Wild => {
                let used: Vec<Key> = rows.iter().filter_map(|r| if let P::Ctor(k, _) = &r[0] { Some(k.clone()) } else { None }).fold(
                    Vec::new(),
                    |mut acc, k| {
                        if !acc.contains(&k) {
                            acc.push(k);
                        }
                        acc
                    },
                );
                let all = self.all_ctors(t0);
                let complete = match &all {
                    Some(all) => !all.is_empty() && all.iter().all(|k| used.contains(k)),
                    None => false,
                };
                if complete {
                    for k in all.unwrap() {
                        let arity = self.arity(&k, t0);
                        let rows2 = self.specialize(rows, &k, arity);
                        let q2: Vec<P> = std::iter::repeat_n(P::Wild, arity).chain(q[1..].iter().cloned()).collect();
                        let tys2: Vec<Ty> = pad(self.field_tys(&k, t0), arity).into_iter().chain(tys[1..].iter().cloned()).collect();
                        if let Some(w) = self.useful(&rows2, &q2, &tys2) {
                            return Some(rebuild(k, arity, w));
                        }
                    }
                    None
                } else {
                    let default: Vec<Vec<P>> = rows.iter().filter(|r| matches!(r[0], P::Wild)).map(|r| r[1..].to_vec()).collect();
                    let w = self.useful(&default, &q[1..], &tys[1..])?;
                    // Name a constructor that no row covers, if the type has a finite set of them.
                    let head = match all.and_then(|all| all.into_iter().find(|k| !used.contains(k))) {
                        Some(k) if !used.is_empty() => {
                            let arity = self.arity(&k, t0);
                            P::Ctor(k, vec![P::Wild; arity])
                        }
                        _ => P::Wild,
                    };
                    Some(std::iter::once(head).chain(w).collect())
                }
            }
        }
    }

    fn show(&self, p: &P, t: &Ty) -> String {
        match p {
            P::Wild => "_".to_string(),
            P::Ctor(k, args) => {
                let ftys = pad(self.field_tys(k, t), args.len());
                let parts: Vec<String> = args.iter().zip(&ftys).map(|(a, ft)| self.show(a, ft)).collect();
                match k {
                    Key::Bool(b) => b.to_string(),
                    Key::Int(n) => n.to_string(),
                    Key::Str(s) => format!("{s:?}"),
                    Key::Tuple(_) => format!("({})", parts.join(", ")),
                    Key::Adt(tag) => {
                        let name = match t {
                            Ty::Con(n, _) => self
                                .typed
                                .adt_index
                                .get(&**n)
                                .map(|&i| self.typed.adts[i].ctors[*tag as usize].name.clone())
                                .unwrap_or_else(|| "?".into()),
                            _ => "?".into(),
                        };
                        if parts.is_empty() { name } else { format!("{name}({})", parts.join(", ")) }
                    }
                }
            }
        }
    }
}

fn pad(mut v: Vec<Ty>, n: usize) -> Vec<Ty> {
    v.resize(n, Ty::Var(u32::MAX));
    v
}

/// Turns a witness for `[fields..., rest...]` back into one for `[K(fields), rest...]`.
fn rebuild(k: Key, arity: usize, w: Vec<P>) -> Vec<P> {
    let mut w = w;
    let rest = w.split_off(arity.min(w.len()));
    std::iter::once(P::Ctor(k, w)).chain(rest).collect()
}
