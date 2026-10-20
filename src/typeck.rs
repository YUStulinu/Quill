//! Hindley–Milner type inference.
//!
//! Top-level functions are checked one strongly connected component of the call graph at a time: inside a
//! component (mutually recursive functions) they are monomorphic, and the whole component is generalized
//! afterwards, so every function can be used polymorphically by the components after it. Local `let`
//! bindings are generalized too when their value is a closure (the value restriction).
//!
//! Besides types, the checker resolves every name (local variable, function, builtin or constructor) and
//! records both in side tables keyed by node id, for lowering and for the language server.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::builtins::{self, Builtin};
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::exhaust;
use crate::types::{Scheme, Ty, TyPrinter, TyTable, UnifyError, subst_gen};

pub type BindingId = u32;

#[derive(Clone, Debug)]
pub struct CtorInfo {
    pub name: String,
    pub tag: u32,
    /// Field types, with `Gen(i)` standing for the type's i-th parameter.
    pub fields: Vec<Ty>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct AdtInfo {
    pub name: String,
    pub params: Vec<String>,
    pub ctors: Vec<CtorInfo>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Res {
    Local(BindingId),
    Global(String),
    Builtin(Builtin),
    /// Constructor: (index of the ADT, tag).
    Ctor(usize, u32),
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub name: String,
    pub span: Span,
    pub mutable: bool,
    pub ty: Ty,
    /// Set for let-bound closures that were generalized.
    pub scheme: Option<Scheme>,
    /// How many closures deep the binding was created.
    pub depth: u32,
    pub uses: u32,
}

#[derive(Clone, Debug)]
pub struct FnInfo {
    pub name: String,
    pub scheme: Scheme,
    pub name_span: Span,
    pub params: Vec<BindingId>,
}

/// The result of type checking: the program plus everything later stages need to know about it.
#[derive(Debug, Default)]
pub struct Typed {
    pub program: Program,
    pub adts: Vec<AdtInfo>,
    pub adt_index: HashMap<String, usize>,
    pub ctor_index: HashMap<String, (usize, u32)>,
    pub fns: HashMap<String, FnInfo>,
    /// Functions in declaration order.
    pub fn_order: Vec<String>,
    /// The type of every expression and pattern.
    pub types: HashMap<NodeId, Ty>,
    /// What every name expression (and assignment target) refers to.
    pub res: HashMap<NodeId, Res>,
    /// The binding introduced by a binding pattern or a parameter.
    pub pat_bind: HashMap<NodeId, BindingId>,
    pub bindings: Vec<Binding>,
    /// Source spans of name uses, binding patterns and parameters (for hover and go-to-definition).
    pub spans: HashMap<NodeId, Span>,
}

impl Typed {
    pub fn ty(&self, id: NodeId) -> Ty {
        self.types.get(&id).cloned().unwrap_or_else(Ty::unit)
    }

    pub fn adt_of_ctor(&self, name: &str) -> Option<(&AdtInfo, &CtorInfo)> {
        let (a, t) = *self.ctor_index.get(name)?;
        Some((&self.adts[a], &self.adts[a].ctors[t as usize]))
    }
}

enum FnSig {
    /// Being inferred, in the current component: monomorphic.
    Mono(Ty),
    Poly(Scheme),
}

struct Checker<'d> {
    tt: TyTable,
    diags: &'d mut Diagnostics,
    out: Typed,
    scopes: Vec<HashMap<String, BindingId>>,
    sigs: HashMap<String, FnSig>,
    ret_stack: Vec<Ty>,
    lambda_depth: u32,
    eq_checks: Vec<(Span, Ty)>,
    matches: Vec<(NodeId, Span)>,
}

pub fn check(program: Program, diags: &mut Diagnostics) -> Typed {
    let mut c = Checker {
        tt: TyTable::default(),
        diags,
        out: Typed::default(),
        scopes: vec![],
        sigs: HashMap::new(),
        ret_stack: vec![],
        lambda_depth: 0,
        eq_checks: vec![],
        matches: vec![],
    };
    c.out.program = program;
    c.run();
    c.out
}

const RESERVED_TYPES: &[&str] = &["Int", "Bool", "String", "Array", "Unit"];

impl<'d> Checker<'d> {
    fn run(&mut self) {
        let program = std::mem::take(&mut self.out.program);
        self.collect_types(&program);
        let fns: Vec<&FnDecl> = program.items.iter().filter_map(|i| if let Item::Fn(f) = i { Some(f) } else { None }).collect();

        // Function names.
        let mut seen: HashMap<&str, Span> = HashMap::new();
        let mut unique: Vec<&FnDecl> = Vec::new();
        for f in &fns {
            if let Some(prev) = seen.get(f.name.name.as_str()) {
                self.diags.push(
                    Diagnostic::error(f.name.span, format!("function `{}` is defined twice", f.name.name))
                        .with_secondary(*prev, "first defined here"),
                );
                continue;
            }
            if builtins::lookup(&f.name.name).is_some() {
                self.diags
                    .push(Diagnostic::error(f.name.span, format!("`{}` is a built-in function and cannot be redefined", f.name.name)));
                continue;
            }
            seen.insert(&f.name.name, f.name.span);
            unique.push(f);
            self.out.fn_order.push(f.name.name.clone());
        }

        // Check one strongly connected component of the call graph at a time.
        let names: HashSet<&str> = unique.iter().map(|f| f.name.name.as_str()).collect();
        let deps: Vec<Vec<usize>> = unique
            .iter()
            .map(|f| {
                let used = free_globals(f, &names);
                unique.iter().enumerate().filter(|(_, g)| used.contains(g.name.name.as_str())).map(|(i, _)| i).collect()
            })
            .collect();
        for scc in tarjan(&deps) {
            self.check_component(&scc.iter().map(|&i| unique[i]).collect::<Vec<_>>());
        }

        match self.out.fns.get("main") {
            None => {
                if !program.items.is_empty() || self.out.fns.is_empty() {
                    self.diags.push(
                        Diagnostic::error(Span::new(0, 0), "no `main` function").with_note("every program starts at `fn main() { ... }`"),
                    );
                }
            }
            Some(m) => {
                if let Ty::Fn(ps, _) = &m.scheme.ty
                    && !ps.is_empty()
                {
                    self.diags.push(Diagnostic::error(m.name_span, "`main` must not take parameters"));
                }
            }
        }

        self.finish(&program);
        self.out.program = program;
    }

    // ------------------------------------------------------------------
    // Type declarations
    // ------------------------------------------------------------------

    fn collect_types(&mut self, program: &Program) {
        let decls: Vec<&TypeDecl> = program.items.iter().filter_map(|i| if let Item::Type(t) = i { Some(t) } else { None }).collect();
        for d in &decls {
            if RESERVED_TYPES.contains(&d.name.name.as_str()) {
                self.diags.error(d.name.span, format!("`{}` is a built-in type", d.name.name));
                continue;
            }
            if let Some(&prev) = self.out.adt_index.get(&d.name.name) {
                let prev_span = self.out.adts[prev].span;
                self.diags.push(
                    Diagnostic::error(d.name.span, format!("type `{}` is defined twice", d.name.name))
                        .with_secondary(prev_span, "first defined here"),
                );
                continue;
            }
            self.out.adt_index.insert(d.name.name.clone(), self.out.adts.len());
            self.out.adts.push(AdtInfo {
                name: d.name.name.clone(),
                params: d.params.iter().map(|p| p.name.clone()).collect(),
                ctors: vec![],
                span: d.name.span,
            });
        }
        for d in &decls {
            let Some(&idx) = self.out.adt_index.get(&d.name.name) else { continue };
            if self.out.adts[idx].span != d.name.span {
                continue;
            }
            let params: HashMap<String, Ty> = d.params.iter().enumerate().map(|(i, p)| (p.name.clone(), Ty::Gen(i as u32))).collect();
            let mut ctors = Vec::new();
            for (tag, c) in d.ctors.iter().enumerate() {
                if let Some(&(other, _)) = self.out.ctor_index.get(&c.name.name) {
                    let prev = self.out.adts[other].ctors.iter().find(|x| x.name == c.name.name).map(|x| x.span);
                    let mut diag = Diagnostic::error(c.name.span, format!("constructor `{}` is defined twice", c.name.name));
                    if let Some(p) = prev {
                        diag = diag.with_secondary(p, "first defined here");
                    }
                    self.diags.push(diag);
                    continue;
                }
                let fields = c.fields.iter().map(|f| self.type_from_ast(f, &mut params.clone(), false)).collect();
                self.out.ctor_index.insert(c.name.name.clone(), (idx, tag as u32));
                ctors.push(CtorInfo { name: c.name.name.clone(), tag: tag as u32, fields, span: c.name.span });
            }
            self.out.adts[idx].ctors = ctors;
        }
    }

    /// Converts a written type. Type variables are looked up in (and, if `allow_new`, added to) `vars`.
    fn type_from_ast(&mut self, t: &TypeExpr, vars: &mut HashMap<String, Ty>, allow_new: bool) -> Ty {
        match t {
            TypeExpr::Var(v) => {
                if let Some(t) = vars.get(&v.name) {
                    return t.clone();
                }
                if allow_new {
                    let fresh = self.tt.fresh();
                    vars.insert(v.name.clone(), fresh.clone());
                    fresh
                } else {
                    self.diags.error(v.span, format!("unknown type variable `{}`", v.name));
                    self.tt.fresh()
                }
            }
            TypeExpr::Named(n, args) => {
                let args: Vec<Ty> = args.iter().map(|a| self.type_from_ast(a, vars, allow_new)).collect();
                let arity = match n.name.as_str() {
                    "Int" | "Bool" | "String" => Some(0),
                    "Unit" => {
                        return Ty::unit();
                    }
                    "Array" => Some(1),
                    other => self.out.adt_index.get(other).map(|&i| self.out.adts[i].params.len()),
                };
                match arity {
                    None => {
                        self.diags.error(n.span, format!("unknown type `{}`", n.name));
                        self.tt.fresh()
                    }
                    Some(k) if k != args.len() => {
                        self.diags.error(
                            n.span,
                            format!(
                                "type `{}` takes {k} type argument{}, but {} {} given",
                                n.name,
                                plural(k),
                                args.len(),
                                if args.len() == 1 { "was" } else { "were" }
                            ),
                        );
                        self.tt.fresh()
                    }
                    Some(_) => Ty::Con(n.name.as_str().into(), args),
                }
            }
            TypeExpr::Fn(ps, r, _) => {
                let ps = ps.iter().map(|p| self.type_from_ast(p, vars, allow_new)).collect();
                let r = self.type_from_ast(r, vars, allow_new);
                Ty::Fn(ps, Box::new(r))
            }
            TypeExpr::Tuple(es, _) => Ty::Tuple(es.iter().map(|e| self.type_from_ast(e, vars, allow_new)).collect()),
        }
    }

    // ------------------------------------------------------------------
    // Functions
    // ------------------------------------------------------------------

    fn check_component(&mut self, fns: &[&FnDecl]) {
        self.tt.enter_level();
        let mut sigs = Vec::new();
        for f in fns {
            let mut vars = HashMap::new();
            let params: Vec<Ty> = f
                .params
                .iter()
                .map(|p| match &p.ty {
                    Some(t) => self.type_from_ast(t, &mut vars, true),
                    None => self.tt.fresh(),
                })
                .collect();
            let ret = match &f.ret {
                Some(t) => self.type_from_ast(t, &mut vars, true),
                None => self.tt.fresh(),
            };
            let ty = Ty::Fn(params.clone(), Box::new(ret.clone()));
            self.sigs.insert(f.name.name.clone(), FnSig::Mono(ty.clone()));
            sigs.push((params, ret, ty));
        }
        for (f, (params, ret, _)) in fns.iter().zip(&sigs) {
            self.scopes.push(HashMap::new());
            let mut ids = Vec::new();
            let mut names = HashSet::new();
            for (p, t) in f.params.iter().zip(params) {
                if p.name.name != "_" && !names.insert(p.name.name.clone()) {
                    self.diags.error(p.name.span, format!("parameter `{}` appears twice", p.name.name));
                }
                let b = self.bind(&p.name.name, p.name.span, t.clone(), false, p.id);
                ids.push(b);
            }
            self.ret_stack.push(ret.clone());
            let body_ty = self.infer(&f.body);
            let span = f.body.span;
            self.expect_ty(ret, &body_ty, tail_span(&f.body).unwrap_or(span), "the function's return type");
            self.ret_stack.pop();
            self.scopes.pop();
            self.out.fns.insert(
                f.name.name.clone(),
                FnInfo { name: f.name.name.clone(), scheme: Scheme::mono(Ty::unit()), name_span: f.name.span, params: ids },
            );
        }
        self.tt.leave_level();
        for (f, (_, _, ty)) in fns.iter().zip(&sigs) {
            let scheme = self.tt.generalize(ty);
            self.sigs.insert(f.name.name.clone(), FnSig::Poly(scheme.clone()));
            if let Some(info) = self.out.fns.get_mut(&f.name.name) {
                info.scheme = scheme;
            }
        }
    }

    fn bind(&mut self, name: &str, span: Span, ty: Ty, mutable: bool, node: NodeId) -> BindingId {
        let id = self.out.bindings.len() as BindingId;
        self.out.bindings.push(Binding { name: name.to_string(), span, mutable, ty, scheme: None, depth: self.lambda_depth, uses: 0 });
        if name != "_" {
            self.scopes.last_mut().expect("a scope is open").insert(name.to_string(), id);
        }
        self.out.pat_bind.insert(node, id);
        self.out.spans.insert(node, span);
        id
    }

    fn lookup_local(&self, name: &str) -> Option<BindingId> {
        self.scopes.iter().rev().find_map(|s| s.get(name).copied())
    }

    // ------------------------------------------------------------------
    // Errors
    // ------------------------------------------------------------------

    /// Unifies `expected` with `found`, reporting a mismatch at `span`.
    fn expect_ty(&mut self, expected: &Ty, found: &Ty, span: Span, what: &str) -> bool {
        match self.tt.unify(expected, found) {
            Ok(()) => true,
            Err(e) => {
                let mut p = TyPrinter::default();
                let exp = p.print(&self.tt.resolve(expected));
                let got = p.print(&self.tt.resolve(found));
                let d = match e {
                    UnifyError::Mismatch => Diagnostic::error(span, format!("mismatched types: expected {exp}, found {got}"))
                        .with_label(format!("this is {}", article(&got)))
                        .with_note(format!("expected because of {what}")),
                    UnifyError::Occurs => Diagnostic::error(span, format!("infinite type: {exp} would have to contain itself ({got})")),
                };
                self.diags.push(d);
                false
            }
        }
    }

    fn unknown_name(&mut self, name: &str, span: Span) {
        let mut candidates: Vec<String> = self.scopes.iter().flat_map(|s| s.keys().cloned()).collect();
        candidates.extend(self.sigs.keys().cloned());
        candidates.extend(builtins::BUILTINS.iter().map(|b| b.name.to_string()));
        candidates.extend(self.out.ctor_index.keys().cloned());
        let mut d = Diagnostic::error(span, format!("cannot find `{name}` in this scope")).with_label("not found");
        if let Some(best) = closest(name, &candidates) {
            d = d.with_note(format!("did you mean `{best}`?"));
        }
        self.diags.push(d);
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn infer(&mut self, e: &Expr) -> Ty {
        let t = self.infer_inner(e);
        self.out.types.insert(e.id, t.clone());
        t
    }

    fn infer_inner(&mut self, e: &Expr) -> Ty {
        match &e.kind {
            ExprKind::Int(_) => Ty::int(),
            ExprKind::Bool(_) => Ty::bool(),
            ExprKind::Str(_) => Ty::string(),
            ExprKind::Unit => Ty::unit(),
            ExprKind::Error => self.tt.fresh(),
            ExprKind::Paren(inner) => self.infer(inner),
            ExprKind::Name(name) => self.infer_name(e, name),
            ExprKind::Tuple(es) => Ty::Tuple(es.iter().map(|x| self.infer(x)).collect()),
            ExprKind::Call(callee, args) => self.infer_call(e, callee, args),
            ExprKind::Unary(op, x) => {
                let t = self.infer(x);
                let want = if *op == UnOp::Neg { Ty::int() } else { Ty::bool() };
                let sym = if *op == UnOp::Neg { "-" } else { "!" };
                self.operand(&want, &t, x.span, sym);
                want
            }
            ExprKind::Binary(op, a, b) => self.infer_binary(*op, a, b),
            ExprKind::If(c, t, f) => {
                let ct = self.infer(c);
                self.operand(&Ty::bool(), &ct, c.span, "if");
                let tt = self.infer(t);
                match f {
                    Some(f) => {
                        let ft = self.infer(f);
                        if self.tt.unify(&tt, &ft).is_err() {
                            let mut p = TyPrinter::default();
                            let a = p.print(&self.tt.resolve(&tt));
                            let b = p.print(&self.tt.resolve(&ft));
                            self.diags.push(
                                Diagnostic::error(
                                    tail_span(f).unwrap_or(f.span),
                                    format!("`if` and `else` have different types: {a} and {b}"),
                                )
                                .with_label(format!("this is {}", article(&b)))
                                .with_secondary(tail_span(t).unwrap_or(t.span), format!("this is {}", article(&a))),
                            );
                        }
                        tt
                    }
                    None => {
                        if self.tt.unify(&tt, &Ty::unit()).is_err() {
                            let got = TyPrinter::default().print(&self.tt.resolve(&tt));
                            self.diags.push(
                                Diagnostic::error(
                                    tail_span(t).unwrap_or(t.span),
                                    format!("`if` without `else` must produce (), but this produces {got}"),
                                )
                                .with_note("add an `else` branch"),
                            );
                        }
                        Ty::unit()
                    }
                }
            }
            ExprKind::Block(stmts, tail) => {
                self.scopes.push(HashMap::new());
                let mut diverges = false;
                for s in stmts {
                    diverges = self.stmt(s);
                }
                let t = match tail {
                    Some(t) => self.infer(t),
                    None if diverges => self.tt.fresh(),
                    None => Ty::unit(),
                };
                self.scopes.pop();
                t
            }
            ExprKind::Lambda(params, body) => {
                self.lambda_depth += 1;
                self.scopes.push(HashMap::new());
                let mut vars = HashMap::new();
                let mut pts = Vec::new();
                for p in params {
                    let t = match &p.ty {
                        Some(t) => self.type_from_ast(t, &mut vars, true),
                        None => self.tt.fresh(),
                    };
                    self.bind(&p.name.name, p.name.span, t.clone(), false, p.id);
                    pts.push(t);
                }
                let ret = self.tt.fresh();
                self.ret_stack.push(ret.clone());
                let bt = self.infer(body);
                self.expect_ty(&ret, &bt, tail_span(body).unwrap_or(body.span), "the closure's return type");
                self.ret_stack.pop();
                self.scopes.pop();
                self.lambda_depth -= 1;
                Ty::Fn(pts, Box::new(ret))
            }
            ExprKind::Match(scrut, arms) => {
                let st = self.infer(scrut);
                let result = self.tt.fresh();
                let mut first: Option<(Span, Ty)> = None;
                for arm in arms {
                    self.scopes.push(HashMap::new());
                    self.pattern(&arm.pat, &st, &mut HashSet::new());
                    if let Some(g) = &arm.guard {
                        let gt = self.infer(g);
                        self.operand(&Ty::bool(), &gt, g.span, "a match guard");
                    }
                    let bt = self.infer(&arm.body);
                    let span = tail_span(&arm.body).unwrap_or(arm.body.span);
                    if self.tt.unify(&result, &bt).is_err() {
                        let mut p = TyPrinter::default();
                        let (fs, ft) = first.clone().unwrap_or((span, result.clone()));
                        let a = p.print(&self.tt.resolve(&ft));
                        let b = p.print(&self.tt.resolve(&bt));
                        self.diags.push(
                            Diagnostic::error(span, format!("match arms have different types: {a} and {b}"))
                                .with_label(format!("this is {}", article(&b)))
                                .with_secondary(fs, format!("the first arm is {}", article(&a))),
                        );
                    }
                    if first.is_none() {
                        first = Some((span, bt));
                    }
                    self.scopes.pop();
                }
                self.matches.push((e.id, e.span));
                result
            }
            ExprKind::Return(v) => {
                let vt = match v {
                    Some(v) => self.infer(v),
                    None => Ty::unit(),
                };
                match self.ret_stack.last().cloned() {
                    Some(r) => {
                        let span = v.as_ref().map_or(e.span, |v| v.span);
                        self.expect_ty(&r, &vt, span, "the function's return type");
                    }
                    None => self.diags.error(e.span, "`return` outside of a function"),
                }
                self.tt.fresh()
            }
        }
    }

    fn operand(&mut self, want: &Ty, got: &Ty, span: Span, op: &str) {
        if self.tt.unify(want, got).is_err() {
            let mut p = TyPrinter::default();
            let w = p.print(want);
            let g = p.print(&self.tt.resolve(got));
            let what = if op == "if" || op.starts_with("a ") { format!("{op} needs {w}") } else { format!("`{op}` needs {w} operands") };
            self.diags.push(
                Diagnostic::error(span, format!("mismatched types: {what}, found {g}")).with_label(format!("this is {}", article(&g))),
            );
        }
    }

    fn infer_binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Ty {
        let at = self.infer(a);
        let bt = self.infer(b);
        let sym = op.symbol();
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => {
                self.operand(&Ty::int(), &at, a.span, sym);
                self.operand(&Ty::int(), &bt, b.span, sym);
                Ty::int()
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                self.operand(&Ty::int(), &at, a.span, sym);
                self.operand(&Ty::int(), &bt, b.span, sym);
                Ty::bool()
            }
            BinOp::And | BinOp::Or => {
                self.operand(&Ty::bool(), &at, a.span, sym);
                self.operand(&Ty::bool(), &bt, b.span, sym);
                Ty::bool()
            }
            BinOp::Concat => {
                self.operand(&Ty::string(), &at, a.span, sym);
                self.operand(&Ty::string(), &bt, b.span, sym);
                Ty::string()
            }
            BinOp::Eq | BinOp::Ne => {
                if self.tt.unify(&at, &bt).is_err() {
                    let mut p = TyPrinter::default();
                    let x = p.print(&self.tt.resolve(&at));
                    let y = p.print(&self.tt.resolve(&bt));
                    self.diags.push(
                        Diagnostic::error(b.span, format!("cannot compare {x} with {y}"))
                            .with_label(format!("this is {}", article(&y)))
                            .with_secondary(a.span, format!("this is {}", article(&x))),
                    );
                }
                self.eq_checks.push((a.span.to(b.span), at));
                Ty::bool()
            }
        }
    }

    fn infer_name(&mut self, e: &Expr, name: &str) -> Ty {
        self.out.spans.insert(e.id, e.span);
        if let Some(b) = self.lookup_local(name) {
            let binding = &mut self.out.bindings[b as usize];
            binding.uses += 1;
            if binding.mutable && binding.depth < self.lambda_depth {
                let span = binding.span;
                self.diags.push(
                    Diagnostic::error(e.span, format!("closures cannot capture the mutable variable `{name}`"))
                        .with_secondary(span, "declared with `var` here")
                        .with_note(format!("copy its current value into a `let` first: `let {name}_now = {name};`")),
                );
            }
            let t = match binding.scheme.clone() {
                Some(s) => self.tt.instantiate(&s),
                None => binding.ty.clone(),
            };
            self.out.res.insert(e.id, Res::Local(b));
            return t;
        }
        if let Some(sig) = self.sigs.get(name) {
            let t = match sig {
                FnSig::Mono(t) => t.clone(),
                FnSig::Poly(s) => {
                    let s = s.clone();
                    self.tt.instantiate(&s)
                }
            };
            self.out.res.insert(e.id, Res::Global(name.to_string()));
            return t;
        }
        if let Some(info) = builtins::lookup(name) {
            self.out.res.insert(e.id, Res::Builtin(info.builtin));
            return self.tt.instantiate(&builtins::scheme(info.builtin));
        }
        if let Some(&(adt, tag)) = self.out.ctor_index.get(name) {
            self.out.res.insert(e.id, Res::Ctor(adt, tag));
            return self.ctor_type(adt, tag);
        }
        self.unknown_name(name, e.span);
        self.tt.fresh()
    }

    /// The type of a constructor used as an expression: the ADT itself for a nullary constructor,
    /// otherwise a function from its fields to the ADT.
    fn ctor_type(&mut self, adt: usize, tag: u32) -> Ty {
        let info = &self.out.adts[adt];
        let n = info.params.len();
        let name = info.name.clone();
        let fields = info.ctors[tag as usize].fields.clone();
        let args: Vec<Ty> = (0..n).map(|_| self.tt.fresh()).collect();
        let result = Ty::Con(name.as_str().into(), args.clone());
        if fields.is_empty() { result } else { Ty::Fn(fields.iter().map(|f| subst_gen(f, &args)).collect(), Box::new(result)) }
    }

    fn infer_call(&mut self, e: &Expr, callee: &Expr, args: &[Expr]) -> Ty {
        let ct = self.infer(callee);
        let ct = self.tt.shallow(&ct);
        let what = match &callee.kind {
            ExprKind::Name(n) => format!("`{n}`"),
            _ => "this function".to_string(),
        };
        match ct {
            Ty::Fn(params, ret) => {
                if params.len() != args.len() {
                    let is_ctor = matches!(self.out.res.get(&callee.id), Some(Res::Ctor(..)));
                    self.diags.push(
                        Diagnostic::error(
                            e.span,
                            format!(
                                "{what} takes {} {}{}, but {} {} given",
                                params.len(),
                                if is_ctor { "field" } else { "argument" },
                                plural(params.len()),
                                args.len(),
                                if args.len() == 1 { "was" } else { "were" }
                            ),
                        )
                        .with_label(format!("expected {} here", params.len())),
                    );
                    for a in args {
                        self.infer(a);
                    }
                    return *ret;
                }
                for (i, (a, p)) in args.iter().zip(&params).enumerate() {
                    let at = self.infer(a);
                    self.expect_ty(p, &at, a.span, &format!("argument {} of {what}", i + 1));
                }
                *ret
            }
            Ty::Var(_) => {
                let ats: Vec<Ty> = args.iter().map(|a| self.infer(a)).collect();
                let ret = self.tt.fresh();
                self.expect_ty(&ct, &Ty::Fn(ats, Box::new(ret.clone())), callee.span, "how it is called");
                ret
            }
            other => {
                let shown = TyPrinter::default().print(&self.tt.resolve(&other));
                self.diags.push(
                    Diagnostic::error(callee.span, format!("{what} is not a function: it is {}", article(&shown)))
                        .with_label("called here"),
                );
                for a in args {
                    self.infer(a);
                }
                self.tt.fresh()
            }
        }
    }

    /// Checks a statement. Returns true if it certainly does not complete (a `return`).
    fn stmt(&mut self, s: &Stmt) -> bool {
        match s {
            Stmt::Expr(e) => {
                self.infer(e);
                matches!(e.kind, ExprKind::Return(_)) || is_panic_call(e)
            }
            Stmt::Let { pat, ty, value, mutable, .. } => {
                let generalize = !mutable && matches!(value.kind, ExprKind::Lambda(..)) && matches!(pat.kind, PatKind::Bind(_));
                if generalize {
                    self.tt.enter_level();
                }
                let mut vt = self.infer(value);
                if let Some(ann) = ty {
                    let at = self.type_from_ast(ann, &mut HashMap::new(), true);
                    self.expect_ty(&at, &vt, value.span, "the type annotation");
                    // The variable has the declared type even if the value does not, so one wrong
                    // annotation reports one error.
                    vt = at;
                }
                if generalize {
                    self.tt.leave_level();
                }
                if !is_irrefutable(&pat.kind, &self.out) {
                    self.diags.push(
                        Diagnostic::error(pat.span, "this pattern can fail to match, so it cannot be used in `let`")
                            .with_note("use `match` instead"),
                    );
                }
                if *mutable {
                    if let PatKind::Bind(name) = &pat.kind {
                        self.bind(name, pat.span, vt.clone(), true, pat.id);
                        self.out.types.insert(pat.id, vt);
                    }
                } else {
                    self.pattern(pat, &vt, &mut HashSet::new());
                    if generalize && let Some(&b) = self.out.pat_bind.get(&pat.id) {
                        let scheme = self.tt.generalize(&vt);
                        if scheme.vars > 0 {
                            self.out.bindings[b as usize].scheme = Some(scheme);
                        }
                    }
                }
                false
            }
            Stmt::Assign { target, target_id, value, .. } => {
                let vt = self.infer(value);
                self.out.spans.insert(*target_id, target.span);
                match self.lookup_local(&target.name) {
                    Some(b) => {
                        self.out.res.insert(*target_id, Res::Local(b));
                        let binding = self.out.bindings[b as usize].clone();
                        if !binding.mutable {
                            self.diags.push(
                                Diagnostic::error(target.span, format!("cannot assign to `{}`: it is not mutable", target.name))
                                    .with_secondary(binding.span, "declared here")
                                    .with_note(format!("declare it with `var {}` to make it mutable", target.name)),
                            );
                        } else if binding.depth < self.lambda_depth {
                            self.diags.push(Diagnostic::error(
                                target.span,
                                format!("closures cannot modify the mutable variable `{}`", target.name),
                            ));
                        }
                        self.expect_ty(&binding.ty, &vt, value.span, &format!("the type of `{}`", target.name));
                        self.out.types.insert(*target_id, binding.ty);
                    }
                    None => self.unknown_name(&target.name, target.span),
                }
                false
            }
            Stmt::While { cond, body, .. } => {
                let ct = self.infer(cond);
                self.operand(&Ty::bool(), &ct, cond.span, "while");
                self.infer(body);
                false
            }
        }
    }

    fn pattern(&mut self, p: &Pattern, expected: &Ty, names: &mut HashSet<String>) {
        self.out.types.insert(p.id, expected.clone());
        match &p.kind {
            PatKind::Wildcard => {}
            PatKind::Bind(name) => {
                if !names.insert(name.clone()) {
                    self.diags.error(p.span, format!("`{name}` is bound twice in the same pattern"));
                }
                self.bind(name, p.span, expected.clone(), false, p.id);
            }
            PatKind::Int(_) => {
                self.expect_ty(expected, &Ty::int(), p.span, "the matched value");
            }
            PatKind::Bool(_) => {
                self.expect_ty(expected, &Ty::bool(), p.span, "the matched value");
            }
            PatKind::Str(_) => {
                self.expect_ty(expected, &Ty::string(), p.span, "the matched value");
            }
            PatKind::Unit => {
                self.expect_ty(expected, &Ty::unit(), p.span, "the matched value");
            }
            PatKind::Tuple(ps) => {
                let ts: Vec<Ty> = ps.iter().map(|_| self.tt.fresh()).collect();
                self.expect_ty(expected, &Ty::Tuple(ts.clone()), p.span, "the matched value");
                for (sp, t) in ps.iter().zip(&ts) {
                    self.pattern(sp, t, names);
                }
            }
            PatKind::Ctor(name, ps) => {
                let Some(&(adt, tag)) = self.out.ctor_index.get(name) else {
                    self.unknown_name(name, p.span);
                    for sp in ps {
                        let t = self.tt.fresh();
                        self.pattern(sp, &t, names);
                    }
                    return;
                };
                self.out.res.insert(p.id, Res::Ctor(adt, tag));
                let info = self.out.adts[adt].clone();
                let fields = &info.ctors[tag as usize].fields;
                if fields.len() != ps.len() {
                    self.diags.error(
                        p.span,
                        format!(
                            "constructor `{name}` has {} field{}, but the pattern has {}",
                            fields.len(),
                            plural(fields.len()),
                            ps.len()
                        ),
                    );
                }
                let args: Vec<Ty> = (0..info.params.len()).map(|_| self.tt.fresh()).collect();
                self.expect_ty(expected, &Ty::Con(info.name.as_str().into(), args.clone()), p.span, "the matched value");
                for (sp, f) in ps.iter().zip(fields) {
                    let ft = subst_gen(f, &args);
                    self.pattern(sp, &ft, names);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // After inference
    // ------------------------------------------------------------------

    fn finish(&mut self, program: &Program) {
        // Resolve every recorded type.
        let types = std::mem::take(&mut self.out.types);
        self.out.types = types.into_iter().map(|(k, v)| (k, self.tt.resolve(&v))).collect();
        for b in &mut self.out.bindings {
            b.ty = self.tt.resolve(&b.ty);
        }
        for (span, t) in std::mem::take(&mut self.eq_checks) {
            if let Ty::Fn(..) = self.tt.resolve(&t) {
                self.diags.push(Diagnostic::error(span, "functions cannot be compared with `==`"));
            }
        }
        // Pattern matches: exhaustiveness and redundancy.
        let matches = std::mem::take(&mut self.matches);
        let mut by_id: HashMap<NodeId, &Expr> = HashMap::new();
        for item in &program.items {
            if let Item::Fn(f) = item {
                collect_matches(&f.body, &mut by_id);
            }
        }
        for (id, span) in matches {
            let Some(e) = by_id.get(&id) else { continue };
            let ExprKind::Match(scrut, arms) = &e.kind else { continue };
            let st = self.out.ty(scrut.id);
            let report = exhaust::check_match(&self.out, &st, arms);
            if let Some(missing) = report.missing {
                self.diags.push(
                    Diagnostic::error(scrut.span, "this `match` does not cover every case")
                        .with_label(format!("`{missing}` is not handled"))
                        .with_secondary(span, "in this match")
                        .with_note("add an arm for it, or a catch-all `_ => ...`"),
                );
            }
            for i in report.unreachable {
                self.diags.push(
                    Diagnostic::warning(arms[i].pat.span, "unreachable pattern")
                        .with_label("the arms above already match every value this would"),
                );
            }
        }
        // Unused variables.
        for b in &self.out.bindings {
            if b.uses == 0 && !b.name.starts_with('_') && b.name != "_" {
                self.diags.push(
                    Diagnostic::warning(b.span, format!("unused variable `{}`", b.name))
                        .with_note(format!("prefix it with an underscore to silence this: `_{}`", b.name)),
                );
            }
        }
    }
}

// ----------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn article(ty: &str) -> String {
    let first = ty.chars().next().unwrap_or('x');
    if ty.starts_with('(') {
        format!("`{ty}`")
    } else if "AEIOU".contains(first) {
        format!("an {ty}")
    } else {
        format!("a {ty}")
    }
}

/// The span of the expression that gives a block its value, for pointing at the right place.
pub fn tail_span(e: &Expr) -> Option<Span> {
    match &e.kind {
        ExprKind::Block(_, Some(t)) => tail_span(t).or(Some(t.span)),
        _ => None,
    }
}

fn is_panic_call(e: &Expr) -> bool {
    matches!(&e.kind, ExprKind::Call(c, _) if matches!(&c.kind, ExprKind::Name(n) if n == "panic"))
}

fn is_irrefutable(p: &PatKind, t: &Typed) -> bool {
    match p {
        PatKind::Wildcard | PatKind::Bind(_) | PatKind::Unit => true,
        PatKind::Tuple(ps) => ps.iter().all(|p| is_irrefutable(&p.kind, t)),
        PatKind::Ctor(name, ps) => {
            t.adt_of_ctor(name).is_some_and(|(adt, _)| adt.ctors.len() == 1) && ps.iter().all(|p| is_irrefutable(&p.kind, t))
        }
        _ => false,
    }
}

fn collect_matches<'a>(e: &'a Expr, out: &mut HashMap<NodeId, &'a Expr>) {
    walk_expr(e, &mut |x| {
        if matches!(x.kind, ExprKind::Match(..)) {
            out.insert(x.id, x);
        }
    });
}

/// Calls `f` on every expression inside `e` (including `e`).
pub fn walk_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Expr)) {
    f(e);
    match &e.kind {
        ExprKind::Call(c, args) => {
            walk_expr(c, f);
            for a in args {
                walk_expr(a, f);
            }
        }
        ExprKind::Binary(_, a, b) => {
            walk_expr(a, f);
            walk_expr(b, f);
        }
        ExprKind::Unary(_, a) | ExprKind::Paren(a) => walk_expr(a, f),
        ExprKind::If(c, t, e2) => {
            walk_expr(c, f);
            walk_expr(t, f);
            if let Some(e2) = e2 {
                walk_expr(e2, f);
            }
        }
        ExprKind::Block(stmts, tail) => {
            for s in stmts {
                match s {
                    Stmt::Let { value, .. } | Stmt::Assign { value, .. } => walk_expr(value, f),
                    Stmt::While { cond, body, .. } => {
                        walk_expr(cond, f);
                        walk_expr(body, f);
                    }
                    Stmt::Expr(x) => walk_expr(x, f),
                }
            }
            if let Some(t) = tail {
                walk_expr(t, f);
            }
        }
        ExprKind::Lambda(_, body) => walk_expr(body, f),
        ExprKind::Match(s, arms) => {
            walk_expr(s, f);
            for a in arms {
                if let Some(g) = &a.guard {
                    walk_expr(g, f);
                }
                walk_expr(&a.body, f);
            }
        }
        ExprKind::Tuple(es) => {
            for x in es {
                walk_expr(x, f);
            }
        }
        ExprKind::Return(Some(v)) => walk_expr(v, f),
        _ => {}
    }
}

/// The top-level functions a function refers to (names not shadowed by a local binding).
fn free_globals(f: &FnDecl, globals: &HashSet<&str>) -> HashSet<String> {
    fn pat_names(p: &Pattern, out: &mut Vec<String>) {
        match &p.kind {
            PatKind::Bind(n) => out.push(n.clone()),
            PatKind::Tuple(ps) | PatKind::Ctor(_, ps) => {
                for q in ps {
                    pat_names(q, out);
                }
            }
            _ => {}
        }
    }
    fn go(e: &Expr, scope: &mut Vec<String>, globals: &HashSet<&str>, out: &mut HashSet<String>) {
        match &e.kind {
            ExprKind::Name(n) => {
                if !scope.contains(n) && globals.contains(n.as_str()) {
                    out.insert(n.clone());
                }
            }
            ExprKind::Block(stmts, tail) => {
                let mark = scope.len();
                for s in stmts {
                    match s {
                        Stmt::Let { pat, value, .. } => {
                            go(value, scope, globals, out);
                            pat_names(pat, scope);
                        }
                        Stmt::Assign { value, .. } => go(value, scope, globals, out),
                        Stmt::While { cond, body, .. } => {
                            go(cond, scope, globals, out);
                            go(body, scope, globals, out);
                        }
                        Stmt::Expr(x) => go(x, scope, globals, out),
                    }
                }
                if let Some(t) = tail {
                    go(t, scope, globals, out);
                }
                scope.truncate(mark);
            }
            ExprKind::Lambda(ps, body) => {
                let mark = scope.len();
                scope.extend(ps.iter().map(|p| p.name.name.clone()));
                go(body, scope, globals, out);
                scope.truncate(mark);
            }
            ExprKind::Match(s, arms) => {
                go(s, scope, globals, out);
                for a in arms {
                    let mark = scope.len();
                    pat_names(&a.pat, scope);
                    if let Some(g) = &a.guard {
                        go(g, scope, globals, out);
                    }
                    go(&a.body, scope, globals, out);
                    scope.truncate(mark);
                }
            }
            ExprKind::Call(c, args) => {
                go(c, scope, globals, out);
                for a in args {
                    go(a, scope, globals, out);
                }
            }
            ExprKind::Binary(_, a, b) => {
                go(a, scope, globals, out);
                go(b, scope, globals, out);
            }
            ExprKind::Unary(_, a) | ExprKind::Paren(a) => go(a, scope, globals, out),
            ExprKind::If(c, t, f) => {
                go(c, scope, globals, out);
                go(t, scope, globals, out);
                if let Some(f) = f {
                    go(f, scope, globals, out);
                }
            }
            ExprKind::Tuple(es) => {
                for x in es {
                    go(x, scope, globals, out);
                }
            }
            ExprKind::Return(Some(v)) => go(v, scope, globals, out),
            _ => {}
        }
    }
    let mut scope: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
    let mut out = HashSet::new();
    go(&f.body, &mut scope, globals, &mut out);
    out
}

/// Tarjan's algorithm. Components come out in reverse topological order: dependencies first.
fn tarjan(deps: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State<'a> {
        deps: &'a [Vec<usize>],
        index: Vec<Option<usize>>,
        low: Vec<usize>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: usize,
        out: Vec<Vec<usize>>,
    }
    fn visit(s: &mut State, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on_stack[v] = true;
        for &w in &s.deps[v] {
            match s.index[w] {
                None => {
                    visit(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(iw) if s.on_stack[w] => s.low[v] = s.low[v].min(iw),
                _ => {}
            }
        }
        if Some(s.low[v]) == s.index[v] {
            let mut comp = Vec::new();
            loop {
                let w = s.stack.pop().unwrap();
                s.on_stack[w] = false;
                comp.push(w);
                if w == v {
                    break;
                }
            }
            comp.sort();
            s.out.push(comp);
        }
    }
    let n = deps.len();
    let mut s = State { deps, index: vec![None; n], low: vec![0; n], on_stack: vec![false; n], stack: vec![], next: 0, out: vec![] };
    for v in 0..n {
        if s.index[v].is_none() {
            visit(&mut s, v);
        }
    }
    s.out
}

/// The candidate closest to `name` by edit distance, if it is close enough to be a plausible typo.
fn closest(name: &str, candidates: &[String]) -> Option<String> {
    fn dist(a: &str, b: &str) -> usize {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for i in 1..=a.len() {
            let mut cur = vec![i; b.len() + 1];
            for j in 1..=b.len() {
                let cost = usize::from(a[i - 1] != b[j - 1]);
                cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            }
            prev = cur;
        }
        prev[b.len()]
    }
    let limit = (name.len() / 3).max(1);
    candidates
        .iter()
        .filter(|c| c.as_str() != name)
        .map(|c| (dist(name, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by_key(|(d, c)| (*d, c.len()))
        .map(|(_, c)| c.clone())
}
