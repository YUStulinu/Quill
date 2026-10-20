//! Types, type schemes and unification (union-find with levels, for efficient let-generalization).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// A unification variable.
    Var(u32),
    /// A quantified variable inside a type scheme.
    Gen(u32),
    /// A named type: `Int`, `Bool`, `String`, `Array[a]`, or a user type.
    Con(Rc<str>, Vec<Ty>),
    Fn(Vec<Ty>, Box<Ty>),
    /// Tuples; the empty tuple is `()`, the unit type.
    Tuple(Vec<Ty>),
}

impl Ty {
    pub fn con(name: &str) -> Ty {
        Ty::Con(Rc::from(name), vec![])
    }

    pub fn int() -> Ty {
        Ty::con("Int")
    }

    pub fn bool() -> Ty {
        Ty::con("Bool")
    }

    pub fn string() -> Ty {
        Ty::con("String")
    }

    pub fn unit() -> Ty {
        Ty::Tuple(vec![])
    }

    pub fn array(elem: Ty) -> Ty {
        Ty::Con(Rc::from("Array"), vec![elem])
    }

    pub fn is_named(&self, name: &str) -> bool {
        matches!(self, Ty::Con(n, args) if &**n == name && args.is_empty())
    }

    pub fn is_unit(&self) -> bool {
        matches!(self, Ty::Tuple(e) if e.is_empty())
    }

    /// Does this type need structural (run-time) equality, or is a word comparison enough?
    pub fn is_immediate(&self) -> bool {
        self.is_named("Int") || self.is_named("Bool") || self.is_unit()
    }
}

/// A polymorphic type: `∀ Gen(0..vars). ty`.
#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub vars: u32,
    pub ty: Ty,
}

impl Scheme {
    pub fn mono(ty: Ty) -> Scheme {
        Scheme { vars: 0, ty }
    }
}

#[derive(Clone, Debug)]
enum VarState {
    Unbound { level: u32 },
    Link(Ty),
}

#[derive(Debug)]
pub enum UnifyError {
    Mismatch,
    Occurs,
}

/// The union-find table of unification variables.
#[derive(Default, Debug)]
pub struct TyTable {
    vars: Vec<VarState>,
    pub level: u32,
}

impl TyTable {
    pub fn fresh(&mut self) -> Ty {
        self.vars.push(VarState::Unbound { level: self.level });
        Ty::Var(self.vars.len() as u32 - 1)
    }

    pub fn enter_level(&mut self) {
        self.level += 1;
    }

    pub fn leave_level(&mut self) {
        self.level -= 1;
    }

    /// Follows links at the top of a type.
    pub fn shallow(&self, t: &Ty) -> Ty {
        let mut t = t.clone();
        while let Ty::Var(v) = t {
            match &self.vars[v as usize] {
                VarState::Link(next) => t = next.clone(),
                VarState::Unbound { .. } => return t,
            }
        }
        t
    }

    /// Replaces every bound variable by what it is bound to.
    pub fn resolve(&self, t: &Ty) -> Ty {
        match self.shallow(t) {
            Ty::Con(n, args) => Ty::Con(n, args.iter().map(|a| self.resolve(a)).collect()),
            Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(|p| self.resolve(p)).collect(), Box::new(self.resolve(&r))),
            Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| self.resolve(e)).collect()),
            other => other,
        }
    }

    fn occurs_and_adjust(&mut self, v: u32, level: u32, t: &Ty) -> Result<(), UnifyError> {
        match self.shallow(t) {
            Ty::Var(w) => {
                if w == v {
                    return Err(UnifyError::Occurs);
                }
                if let VarState::Unbound { level: l } = &mut self.vars[w as usize] {
                    // The variable escapes into a type of an outer level: it must not be generalized there.
                    if *l > level {
                        *l = level;
                    }
                }
                Ok(())
            }
            Ty::Con(_, args) => args.iter().try_for_each(|a| self.occurs_and_adjust(v, level, a)),
            Ty::Fn(ps, r) => {
                ps.iter().try_for_each(|p| self.occurs_and_adjust(v, level, p))?;
                self.occurs_and_adjust(v, level, &r)
            }
            Ty::Tuple(es) => es.iter().try_for_each(|e| self.occurs_and_adjust(v, level, e)),
            Ty::Gen(_) => Ok(()),
        }
    }

    pub fn unify(&mut self, a: &Ty, b: &Ty) -> Result<(), UnifyError> {
        let a = self.shallow(a);
        let b = self.shallow(b);
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), _) => self.bind(*x, &b),
            (_, Ty::Var(y)) => self.bind(*y, &a),
            (Ty::Con(n1, a1), Ty::Con(n2, a2)) => {
                if n1 != n2 || a1.len() != a2.len() {
                    return Err(UnifyError::Mismatch);
                }
                for (x, y) in a1.iter().zip(a2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            (Ty::Fn(p1, r1), Ty::Fn(p2, r2)) => {
                if p1.len() != p2.len() {
                    return Err(UnifyError::Mismatch);
                }
                for (x, y) in p1.iter().zip(p2) {
                    self.unify(x, y)?;
                }
                self.unify(r1, r2)
            }
            (Ty::Tuple(e1), Ty::Tuple(e2)) => {
                if e1.len() != e2.len() {
                    return Err(UnifyError::Mismatch);
                }
                for (x, y) in e1.iter().zip(e2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
            _ => Err(UnifyError::Mismatch),
        }
    }

    fn bind(&mut self, v: u32, t: &Ty) -> Result<(), UnifyError> {
        let level = match self.vars[v as usize] {
            VarState::Unbound { level } => level,
            VarState::Link(_) => unreachable!("bind on a linked variable"),
        };
        self.occurs_and_adjust(v, level, t)?;
        self.vars[v as usize] = VarState::Link(t.clone());
        Ok(())
    }

    /// Quantifies the variables created at a deeper level than the current one.
    pub fn generalize(&self, t: &Ty) -> Scheme {
        let mut map: HashMap<u32, u32> = HashMap::new();
        let ty = self.gen_rec(t, &mut map);
        Scheme { vars: map.len() as u32, ty }
    }

    fn gen_rec(&self, t: &Ty, map: &mut HashMap<u32, u32>) -> Ty {
        match self.shallow(t) {
            Ty::Var(v) => match self.vars[v as usize] {
                VarState::Unbound { level } if level > self.level => {
                    let n = map.len() as u32;
                    Ty::Gen(*map.entry(v).or_insert(n))
                }
                _ => Ty::Var(v),
            },
            Ty::Con(n, args) => Ty::Con(n, args.iter().map(|a| self.gen_rec(a, map)).collect()),
            Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(|p| self.gen_rec(p, map)).collect(), Box::new(self.gen_rec(&r, map))),
            Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| self.gen_rec(e, map)).collect()),
            Ty::Gen(g) => Ty::Gen(g),
        }
    }

    pub fn instantiate(&mut self, s: &Scheme) -> Ty {
        if s.vars == 0 {
            return s.ty.clone();
        }
        let fresh: Vec<Ty> = (0..s.vars).map(|_| self.fresh()).collect();
        subst_gen(&s.ty, &fresh)
    }
}

/// Replaces `Gen(i)` by `args[i]`.
pub fn subst_gen(t: &Ty, args: &[Ty]) -> Ty {
    match t {
        Ty::Gen(i) => args[*i as usize].clone(),
        Ty::Con(n, a) => Ty::Con(n.clone(), a.iter().map(|x| subst_gen(x, args)).collect()),
        Ty::Fn(ps, r) => Ty::Fn(ps.iter().map(|p| subst_gen(p, args)).collect(), Box::new(subst_gen(r, args))),
        Ty::Tuple(es) => Ty::Tuple(es.iter().map(|e| subst_gen(e, args)).collect()),
        Ty::Var(v) => Ty::Var(*v),
    }
}

/// Prints types for humans. Variables get the names `a`, `b`, ... in order of appearance, consistently
/// across all types printed with the same printer (so "expected List[a], found a" is meaningful).
#[derive(Default)]
pub struct TyPrinter {
    names: HashMap<(bool, u32), String>,
}

impl TyPrinter {
    fn var_name(&mut self, key: (bool, u32)) -> String {
        let n = self.names.len();
        self.names
            .entry(key)
            .or_insert_with(|| {
                let letter = (b'a' + (n % 26) as u8) as char;
                if n < 26 { letter.to_string() } else { format!("{letter}{}", n / 26) }
            })
            .clone()
    }

    pub fn print(&mut self, t: &Ty) -> String {
        let mut s = String::new();
        self.write(&mut s, t);
        s
    }

    fn write(&mut self, s: &mut String, t: &Ty) {
        match t {
            Ty::Var(v) => {
                let n = self.var_name((false, *v));
                s.push_str(&n);
            }
            Ty::Gen(g) => {
                let n = self.var_name((true, *g));
                s.push_str(&n);
            }
            Ty::Con(n, args) => {
                s.push_str(n);
                if !args.is_empty() {
                    s.push('[');
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            s.push_str(", ");
                        }
                        self.write(s, a);
                    }
                    s.push(']');
                }
            }
            Ty::Fn(ps, r) => {
                s.push('(');
                for (i, p) in ps.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.write(s, p);
                }
                s.push_str(") -> ");
                self.write(s, r);
            }
            Ty::Tuple(es) => {
                s.push('(');
                for (i, e) in es.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    self.write(s, e);
                }
                s.push(')');
            }
        }
    }
}

pub fn show(t: &Ty) -> String {
    TyPrinter::default().print(t)
}

pub fn show_scheme(s: &Scheme) -> String {
    let mut out = String::new();
    let _ = write!(out, "{}", show(&s.ty));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unifies_and_generalizes() {
        let mut t = TyTable::default();
        t.enter_level();
        let a = t.fresh();
        let f = Ty::Fn(vec![a.clone()], Box::new(a.clone()));
        t.leave_level();
        let s = t.generalize(&f);
        assert_eq!(s.vars, 1);
        assert_eq!(show(&s.ty), "(a) -> a");
        let inst = t.instantiate(&s);
        let r = t.fresh();
        t.unify(&inst, &Ty::Fn(vec![Ty::int()], Box::new(r))).unwrap();
        assert_eq!(show(&t.resolve(&inst)), "(Int) -> Int");
    }

    #[test]
    fn occurs_check() {
        let mut t = TyTable::default();
        let a = t.fresh();
        assert!(matches!(t.unify(&a, &Ty::array(a.clone())), Err(UnifyError::Occurs)));
    }

    #[test]
    fn escaping_variables_are_not_generalized() {
        let mut t = TyTable::default();
        let outer = t.fresh(); // level 0
        t.enter_level();
        let inner = t.fresh();
        t.unify(&inner, &outer).unwrap();
        t.leave_level();
        assert_eq!(t.generalize(&inner).vars, 0);
    }
}
