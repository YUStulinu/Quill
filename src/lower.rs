//! Lowering from the typed AST to the SSA intermediate representation.
//!
//! * Closures are converted: every lambda becomes a function whose first parameter is its environment (the
//!   closure object itself), and its free variables are loaded from that object. Top-level functions,
//!   builtins and constructors used as values get small wrapper functions with the same convention.
//! * Pattern matching becomes a chain of tests: each arm tests its pattern and falls through to the next.
//! * Mutable variables are put in SSA form on the fly with the algorithm of Braun et al. ("Simple and
//!   Efficient Construction of Static Single Assignment Form", CC 2013): reading a variable looks for its
//!   definition in the current block, then in the predecessors, adding block parameters where control flow
//!   merges. Immutable bindings need none of that: their definition dominates every use.
//!
//! Invariant: branch targets never take arguments (every branch goes to a fresh block that then jumps),
//! so all block parameters receive their values from jumps.

use std::collections::{HashMap, HashSet};

use crate::ast::*;
use crate::builtins::{self, Builtin};
use crate::diag::SourceFile;
use crate::ir::{self, BlockId, FuncId, Function, Inst, Module, Target, Term, V};
use crate::typeck::{BindingId, Res, Typed, walk_expr};
use crate::types::Ty;

pub fn lower(t: &Typed, file: &SourceFile) -> Module {
    let mut l =
        Lowerer { t, file, m: Module::default(), fn_ids: HashMap::new(), wrappers: HashMap::new(), ctor0: HashMap::new(), lambdas: 0 };
    for (adt_i, adt) in t.adts.iter().enumerate() {
        for c in &adt.ctors {
            if c.fields.is_empty() {
                let id = l.m.ctor0.len() as u32;
                l.m.ctor0.push(ir::Ctor0Info { adt: adt.name.clone(), name: c.name.clone(), tag: c.tag });
                l.ctor0.insert((adt_i, c.tag), id);
            }
        }
    }
    let decls: Vec<&FnDecl> = t.program.items.iter().filter_map(|i| if let Item::Fn(f) = i { Some(f) } else { None }).collect();
    for name in &t.fn_order {
        let id = l.m.funcs.len() as FuncId;
        l.m.funcs.push(Function::new(name.clone(), false));
        l.fn_ids.insert(name.clone(), id);
    }
    for name in t.fn_order.clone() {
        let decl = decls.iter().find(|d| d.name.name == name).expect("checked functions have declarations");
        let id = l.fn_ids[&name];
        let f = l.lower_fn(decl);
        l.m.funcs[id as usize] = f;
    }
    l.m.main = l.fn_ids.get("main").copied().unwrap_or(0);
    l.m
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum WrapperKey {
    Global(String),
    Builtin(Builtin),
    Ctor(usize, u32),
}

struct Lowerer<'a> {
    t: &'a Typed,
    file: &'a SourceFile,
    m: Module,
    fn_ids: HashMap<String, FuncId>,
    wrappers: HashMap<WrapperKey, FuncId>,
    ctor0: HashMap<(usize, u32), u32>,
    lambdas: u32,
}

/// Builds one IR function.
struct Fb {
    f: Function,
    cur: BlockId,
    vals: HashMap<BindingId, V>,
    mutable: HashSet<BindingId>,
    defs: HashMap<(BindingId, BlockId), V>,
    sealed: HashSet<BlockId>,
    incomplete: HashMap<BlockId, Vec<(BindingId, V)>>,
    preds: HashMap<BlockId, Vec<BlockId>>,
}

impl Fb {
    fn new(name: String, takes_env: bool) -> Fb {
        let f = Function::new(name, takes_env);
        let entry = f.entry;
        let mut sealed = HashSet::new();
        sealed.insert(entry);
        Fb {
            f,
            cur: entry,
            vals: HashMap::new(),
            mutable: HashSet::new(),
            defs: HashMap::new(),
            sealed,
            incomplete: HashMap::new(),
            preds: HashMap::new(),
        }
    }

    fn emit(&mut self, i: Inst) -> V {
        self.f.push(self.cur, i)
    }

    fn konst(&mut self, n: i64) -> V {
        self.emit(Inst::Const(n))
    }

    fn block(&mut self) -> BlockId {
        self.f.new_block()
    }

    fn terminate(&mut self, term: Term) {
        for s in term.successors() {
            self.preds.entry(s).or_default().push(self.cur);
        }
        self.f.blocks[self.cur as usize].term = term;
    }

    fn jump(&mut self, to: BlockId, args: Vec<V>) {
        self.terminate(Term::Jump(Target { block: to, args }));
    }

    /// Branches through fresh single-predecessor blocks, which jump on to `t` and `e`.
    fn branch(&mut self, cond: V, t: BlockId, e: BlockId) {
        let te = self.block();
        let ee = self.block();
        self.terminate(Term::Branch(cond, Target { block: te, args: vec![] }, Target { block: ee, args: vec![] }));
        for (edge, to) in [(te, t), (ee, e)] {
            self.sealed.insert(edge);
            self.cur = edge;
            self.jump(to, vec![]);
        }
    }

    fn switch_to(&mut self, b: BlockId) {
        self.cur = b;
    }

    /// Continues in a fresh block that nothing jumps to (after `return`).
    fn dead_block(&mut self) {
        let b = self.block();
        self.sealed.insert(b);
        self.cur = b;
    }

    // Braun et al. SSA construction, for mutable variables.

    fn write_var(&mut self, var: BindingId, block: BlockId, v: V) {
        self.defs.insert((var, block), v);
    }

    fn read_var(&mut self, var: BindingId, block: BlockId) -> V {
        if let Some(&v) = self.defs.get(&(var, block)) {
            return v;
        }
        let v = if !self.sealed.contains(&block) {
            let p = self.f.add_param(block);
            self.incomplete.entry(block).or_default().push((var, p));
            p
        } else {
            let preds = self.preds.get(&block).cloned().unwrap_or_default();
            match preds.len() {
                0 => self.f.push(block, Inst::Const(0)),
                1 => self.read_var(var, preds[0]),
                _ => {
                    let p = self.f.add_param(block);
                    self.write_var(var, block, p);
                    self.add_phi_args(var, block);
                    p
                }
            }
        };
        self.write_var(var, block, v);
        v
    }

    fn add_phi_args(&mut self, var: BindingId, block: BlockId) {
        for pred in self.preds.get(&block).cloned().unwrap_or_default() {
            let a = self.read_var(var, pred);
            match &mut self.f.blocks[pred as usize].term {
                Term::Jump(t) if t.block == block => t.args.push(a),
                other => panic!("predecessor b{pred} of b{block} does not end in a jump: {other:?}"),
            }
        }
    }

    fn seal(&mut self, block: BlockId) {
        if let Some(list) = self.incomplete.remove(&block) {
            for (var, _) in list {
                self.add_phi_args(var, block);
            }
        }
        self.sealed.insert(block);
    }
}

impl<'a> Lowerer<'a> {
    fn lower_fn(&mut self, d: &FnDecl) -> Function {
        let info = &self.t.fns[&d.name.name];
        let mut fb = Fb::new(d.name.name.clone(), false);
        for &b in &info.params {
            let p = fb.f.add_param(fb.f.entry);
            fb.vals.insert(b, p);
        }
        let v = self.expr(&mut fb, &d.body);
        fb.terminate(Term::Return(v));
        fb.f
    }

    fn func_arity(&self, name: &str) -> usize {
        match &self.t.fns[name].scheme.ty {
            Ty::Fn(ps, _) => ps.len(),
            _ => 0,
        }
    }

    /// A function taking an environment that forwards to a global function, builtin or constructor.
    fn wrapper(&mut self, key: WrapperKey) -> FuncId {
        if let Some(&id) = self.wrappers.get(&key) {
            return id;
        }
        let (name, arity) = match &key {
            WrapperKey::Global(n) => (format!("{n}$clo"), self.func_arity(n)),
            WrapperKey::Builtin(b) => (format!("{}$clo", builtins::info(*b).name), builtins::arity(*b)),
            WrapperKey::Ctor(a, t) => {
                (format!("{}$clo", self.t.adts[*a].ctors[*t as usize].name), self.t.adts[*a].ctors[*t as usize].fields.len())
            }
        };
        let mut fb = Fb::new(name, true);
        let entry = fb.f.entry;
        fb.f.add_param(entry);
        let args: Vec<V> = (0..arity).map(|_| fb.f.add_param(entry)).collect();
        let v = match &key {
            WrapperKey::Global(n) => fb.emit(Inst::Call(self.fn_ids[n], args)),
            WrapperKey::Builtin(b) => fb.emit(Inst::CallBuiltin(*b, args)),
            WrapperKey::Ctor(_, t) => fb.emit(Inst::Alloc { tag: *t, fields: args }),
        };
        fb.terminate(Term::Return(v));
        let id = self.m.funcs.len() as FuncId;
        self.m.funcs.push(fb.f);
        self.wrappers.insert(key, id);
        id
    }

    fn location(&self, span: crate::diag::Span) -> String {
        let (l, c) = self.file.line_col(span.start as usize);
        format!("{}:{}:{}", self.file.name, l + 1, c + 1)
    }

    fn read_binding(&mut self, fb: &mut Fb, b: BindingId) -> V {
        if fb.mutable.contains(&b) {
            let cur = fb.cur;
            fb.read_var(b, cur)
        } else {
            *fb.vals.get(&b).unwrap_or_else(|| panic!("binding {b} ({}) has no value", self.t.bindings[b as usize].name))
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn expr(&mut self, fb: &mut Fb, e: &Expr) -> V {
        match &e.kind {
            ExprKind::Int(n) => fb.konst(*n),
            ExprKind::Bool(b) => fb.konst(i64::from(*b)),
            ExprKind::Unit | ExprKind::Error => fb.konst(0),
            ExprKind::Str(s) => {
                let i = self.m.intern_string(s);
                fb.emit(Inst::Str(i))
            }
            ExprKind::Paren(inner) => self.expr(fb, inner),
            ExprKind::Name(_) => self.name_value(fb, e),
            ExprKind::Tuple(es) => {
                let fields: Vec<V> = es.iter().map(|x| self.expr(fb, x)).collect();
                if fields.is_empty() { fb.konst(0) } else { fb.emit(Inst::Alloc { tag: 0, fields }) }
            }
            ExprKind::Call(callee, args) => self.call(fb, callee, args),
            ExprKind::Unary(op, x) => {
                let v = self.expr(fb, x);
                fb.emit(Inst::Unary(if *op == UnOp::Neg { ir::UnOp::Neg } else { ir::UnOp::Not }, v))
            }
            ExprKind::Binary(op, a, b) => self.binary(fb, *op, a, b),
            ExprKind::If(c, t, f) => {
                let cv = self.expr(fb, c);
                let tb = fb.block();
                let eb = fb.block();
                let join = fb.block();
                fb.branch(cv, tb, eb);
                fb.seal(tb);
                fb.seal(eb);
                let result = fb.f.add_param(join);
                fb.switch_to(tb);
                let tv = self.expr(fb, t);
                fb.jump(join, vec![tv]);
                fb.switch_to(eb);
                let ev = match f {
                    Some(f) => self.expr(fb, f),
                    None => fb.konst(0),
                };
                fb.jump(join, vec![ev]);
                fb.seal(join);
                fb.switch_to(join);
                result
            }
            ExprKind::Block(stmts, tail) => {
                for s in stmts {
                    self.stmt(fb, s);
                }
                match tail {
                    Some(t) => self.expr(fb, t),
                    None => fb.konst(0),
                }
            }
            ExprKind::Lambda(params, body) => self.lambda(fb, e, params, body),
            ExprKind::Match(scrut, arms) => self.match_expr(fb, e, scrut, arms),
            ExprKind::Return(v) => {
                let rv = match v {
                    Some(v) => self.expr(fb, v),
                    None => fb.konst(0),
                };
                fb.terminate(Term::Return(rv));
                fb.dead_block();
                fb.konst(0)
            }
        }
    }

    fn name_value(&mut self, fb: &mut Fb, e: &Expr) -> V {
        match self.t.res.get(&e.id).cloned() {
            Some(Res::Local(b)) => self.read_binding(fb, b),
            Some(Res::Global(n)) => {
                let w = self.wrapper(WrapperKey::Global(n));
                fb.emit(Inst::FuncRef(w))
            }
            Some(Res::Builtin(b)) => {
                let w = self.wrapper(WrapperKey::Builtin(b));
                fb.emit(Inst::FuncRef(w))
            }
            Some(Res::Ctor(adt, tag)) => {
                if let Some(&id) = self.ctor0.get(&(adt, tag)) {
                    fb.emit(Inst::Ctor0(id))
                } else {
                    let w = self.wrapper(WrapperKey::Ctor(adt, tag));
                    fb.emit(Inst::FuncRef(w))
                }
            }
            None => fb.konst(0),
        }
    }

    fn call(&mut self, fb: &mut Fb, callee: &Expr, args: &[Expr]) -> V {
        if let ExprKind::Name(_) = &callee.kind {
            match self.t.res.get(&callee.id).cloned() {
                Some(Res::Global(n)) => {
                    let vs: Vec<V> = args.iter().map(|a| self.expr(fb, a)).collect();
                    return fb.emit(Inst::Call(self.fn_ids[&n], vs));
                }
                Some(Res::Builtin(b)) => {
                    let vs: Vec<V> = args.iter().map(|a| self.expr(fb, a)).collect();
                    return fb.emit(Inst::CallBuiltin(b, vs));
                }
                Some(Res::Ctor(_, tag)) => {
                    let vs: Vec<V> = args.iter().map(|a| self.expr(fb, a)).collect();
                    return fb.emit(Inst::Alloc { tag, fields: vs });
                }
                _ => {}
            }
        }
        let c = self.expr(fb, callee);
        let vs: Vec<V> = args.iter().map(|a| self.expr(fb, a)).collect();
        fb.emit(Inst::CallClosure(c, vs))
    }

    fn binary(&mut self, fb: &mut Fb, op: BinOp, a: &Expr, b: &Expr) -> V {
        match op {
            BinOp::And | BinOp::Or => {
                // a && b  =  if a { b } else { false };   a || b  =  if a { true } else { b }
                let av = self.expr(fb, a);
                let rhs = fb.block();
                let short = fb.block();
                let join = fb.block();
                if op == BinOp::And {
                    fb.branch(av, rhs, short)
                } else {
                    fb.branch(av, short, rhs)
                }
                fb.seal(rhs);
                fb.seal(short);
                let result = fb.f.add_param(join);
                fb.switch_to(rhs);
                let bv = self.expr(fb, b);
                fb.jump(join, vec![bv]);
                fb.switch_to(short);
                let k = fb.konst(i64::from(op == BinOp::Or));
                fb.jump(join, vec![k]);
                fb.seal(join);
                fb.switch_to(join);
                result
            }
            BinOp::Concat => {
                let av = self.expr(fb, a);
                let bv = self.expr(fb, b);
                fb.emit(Inst::CallBuiltin(Builtin::Concat, vec![av, bv]))
            }
            BinOp::Eq | BinOp::Ne => {
                let av = self.expr(fb, a);
                let bv = self.expr(fb, b);
                if self.t.ty(a.id).is_immediate() {
                    fb.emit(Inst::Binary(if op == BinOp::Eq { ir::BinOp::Eq } else { ir::BinOp::Ne }, av, bv))
                } else {
                    let eq = fb.emit(Inst::CallBuiltin(Builtin::Equal, vec![av, bv]));
                    if op == BinOp::Eq { eq } else { fb.emit(Inst::Unary(ir::UnOp::Not, eq)) }
                }
            }
            _ => {
                let av = self.expr(fb, a);
                let bv = self.expr(fb, b);
                let iop = match op {
                    BinOp::Add => ir::BinOp::Add,
                    BinOp::Sub => ir::BinOp::Sub,
                    BinOp::Mul => ir::BinOp::Mul,
                    BinOp::Div => ir::BinOp::Div,
                    BinOp::Mod => ir::BinOp::Mod,
                    BinOp::Lt => ir::BinOp::Lt,
                    BinOp::Le => ir::BinOp::Le,
                    BinOp::Gt => ir::BinOp::Gt,
                    BinOp::Ge => ir::BinOp::Ge,
                    _ => unreachable!(),
                };
                fb.emit(Inst::Binary(iop, av, bv))
            }
        }
    }

    // ------------------------------------------------------------------
    // Statements
    // ------------------------------------------------------------------

    fn stmt(&mut self, fb: &mut Fb, s: &Stmt) {
        match s {
            Stmt::Expr(e) => {
                self.expr(fb, e);
            }
            Stmt::Let { pat, value, mutable, .. } => {
                let v = self.expr(fb, value);
                if *mutable {
                    if let Some(&b) = self.t.pat_bind.get(&pat.id) {
                        fb.mutable.insert(b);
                        let cur = fb.cur;
                        fb.write_var(b, cur, v);
                    }
                } else {
                    self.bind_irrefutable(fb, pat, v);
                }
            }
            Stmt::Assign { target_id, value, .. } => {
                let v = self.expr(fb, value);
                if let Some(Res::Local(b)) = self.t.res.get(target_id) {
                    let cur = fb.cur;
                    fb.write_var(*b, cur, v);
                }
            }
            Stmt::While { cond, body, .. } => {
                let header = fb.block();
                let body_b = fb.block();
                let exit = fb.block();
                fb.jump(header, vec![]);
                fb.switch_to(header);
                let cv = self.expr(fb, cond);
                fb.branch(cv, body_b, exit);
                fb.seal(body_b);
                fb.seal(exit);
                fb.switch_to(body_b);
                self.expr(fb, body);
                fb.jump(header, vec![]);
                fb.seal(header);
                fb.switch_to(exit);
            }
        }
    }

    /// Binds the variables of a pattern that cannot fail (as checked by the type checker).
    fn bind_irrefutable(&mut self, fb: &mut Fb, p: &Pattern, v: V) {
        match &p.kind {
            PatKind::Bind(_) => {
                if let Some(&b) = self.t.pat_bind.get(&p.id) {
                    fb.vals.insert(b, v);
                }
            }
            PatKind::Tuple(ps) | PatKind::Ctor(_, ps) => {
                for (i, q) in ps.iter().enumerate() {
                    if needs_value(q) {
                        let f = fb.emit(Inst::Field(v, i as u32));
                        self.bind_irrefutable(fb, q, f);
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Closures
    // ------------------------------------------------------------------

    fn lambda(&mut self, fb: &mut Fb, e: &Expr, params: &[Param], body: &Expr) -> V {
        // Free variables: locals used inside but bound outside.
        let mut inside: HashSet<BindingId> = params.iter().filter_map(|p| self.t.pat_bind.get(&p.id).copied()).collect();
        let mut used: Vec<BindingId> = Vec::new();
        collect_bindings(body, self.t, &mut inside, &mut used);
        let mut captures: Vec<BindingId> = used.into_iter().filter(|b| !inside.contains(b)).collect();
        captures.sort();
        captures.dedup();

        self.lambdas += 1;
        let (line, _) = self.file.line_col(e.span.start as usize);
        let mut inner = Fb::new(format!("{}$lambda{}_line{}", fb.f.name.split('$').next().unwrap_or(""), self.lambdas, line + 1), true);
        let entry = inner.f.entry;
        let env = inner.f.add_param(entry);
        for p in params {
            let v = inner.f.add_param(entry);
            if let Some(&b) = self.t.pat_bind.get(&p.id) {
                inner.vals.insert(b, v);
            }
        }
        for (i, &b) in captures.iter().enumerate() {
            let v = inner.emit(Inst::Field(env, i as u32 + 1));
            inner.vals.insert(b, v);
        }
        let rv = self.expr(&mut inner, body);
        inner.terminate(Term::Return(rv));
        let id = self.m.funcs.len() as FuncId;
        self.m.funcs.push(inner.f);

        if captures.is_empty() {
            fb.emit(Inst::FuncRef(id))
        } else {
            let vals: Vec<V> = captures.iter().map(|&b| self.read_binding(fb, b)).collect();
            fb.emit(Inst::MakeClosure { func: id, captures: vals })
        }
    }

    // ------------------------------------------------------------------
    // Pattern matching
    // ------------------------------------------------------------------

    fn match_expr(&mut self, fb: &mut Fb, e: &Expr, scrut: &Expr, arms: &[Arm]) -> V {
        let sv = self.expr(fb, scrut);
        let join = fb.block();
        let result = fb.f.add_param(join);
        for arm in arms {
            let next = fb.block();
            self.test_pattern(fb, &arm.pat, sv, next);
            if let Some(g) = &arm.guard {
                let gv = self.expr(fb, g);
                let ok = fb.block();
                fb.branch(gv, ok, next);
                fb.seal(ok);
                fb.switch_to(ok);
            }
            let bv = self.expr(fb, &arm.body);
            fb.jump(join, vec![bv]);
            fb.seal(next);
            fb.switch_to(next);
        }
        let msg = format!("no match arm matched the value (match at {})", self.location(e.span));
        let s = self.m.intern_string(&msg);
        fb.terminate(Term::Panic(s));
        fb.seal(join);
        fb.switch_to(join);
        result
    }

    /// Emits the tests of a pattern against `v`: on failure control goes to `fail`; on success it continues
    /// in the current block, with the pattern's variables bound.
    fn test_pattern(&mut self, fb: &mut Fb, p: &Pattern, v: V, fail: BlockId) {
        let cont = |fb: &mut Fb, cond: V, expect: bool| {
            let ok = fb.block();
            if expect {
                fb.branch(cond, ok, fail)
            } else {
                fb.branch(cond, fail, ok)
            }
            fb.seal(ok);
            fb.switch_to(ok);
        };
        match &p.kind {
            PatKind::Wildcard | PatKind::Unit => {}
            PatKind::Bind(_) => {
                if let Some(&b) = self.t.pat_bind.get(&p.id) {
                    fb.vals.insert(b, v);
                }
            }
            PatKind::Int(n) => {
                let k = fb.konst(*n);
                let c = fb.emit(Inst::Binary(ir::BinOp::Eq, v, k));
                cont(fb, c, true);
            }
            PatKind::Bool(b) => cont(fb, v, *b),
            PatKind::Str(s) => {
                let i = self.m.intern_string(s);
                let k = fb.emit(Inst::Str(i));
                let c = fb.emit(Inst::CallBuiltin(Builtin::Equal, vec![v, k]));
                cont(fb, c, true);
            }
            PatKind::Tuple(ps) => {
                for (i, q) in ps.iter().enumerate() {
                    if needs_value(q) {
                        let f = fb.emit(Inst::Field(v, i as u32));
                        self.test_pattern(fb, q, f, fail);
                    }
                }
            }
            PatKind::Ctor(name, ps) => {
                let Some((adt, ctor)) = self.t.adt_of_ctor(name) else { return };
                if adt.ctors.len() > 1 {
                    let tag = fb.emit(Inst::Tag(v));
                    let k = fb.konst(i64::from(ctor.tag));
                    let c = fb.emit(Inst::Binary(ir::BinOp::Eq, tag, k));
                    cont(fb, c, true);
                }
                for (i, q) in ps.iter().enumerate() {
                    if needs_value(q) {
                        let f = fb.emit(Inst::Field(v, i as u32));
                        self.test_pattern(fb, q, f, fail);
                    }
                }
            }
        }
    }
}

/// Does matching this pattern look at the value at all?
fn needs_value(p: &Pattern) -> bool {
    !matches!(p.kind, PatKind::Wildcard | PatKind::Unit)
}

/// Bindings introduced inside an expression (added to `inside`) and local bindings it refers to (`used`).
fn collect_bindings(e: &Expr, t: &Typed, inside: &mut HashSet<BindingId>, used: &mut Vec<BindingId>) {
    fn pats(p: &Pattern, t: &Typed, inside: &mut HashSet<BindingId>) {
        if let Some(&b) = t.pat_bind.get(&p.id) {
            inside.insert(b);
        }
        if let PatKind::Tuple(ps) | PatKind::Ctor(_, ps) = &p.kind {
            for q in ps {
                pats(q, t, inside);
            }
        }
    }
    walk_expr(e, &mut |x| match &x.kind {
        ExprKind::Name(_) => {
            if let Some(Res::Local(b)) = t.res.get(&x.id) {
                used.push(*b);
            }
        }
        ExprKind::Lambda(ps, _) => {
            for p in ps {
                if let Some(&b) = t.pat_bind.get(&p.id) {
                    inside.insert(b);
                }
            }
        }
        ExprKind::Block(stmts, _) => {
            for s in stmts {
                if let Stmt::Let { pat, .. } = s {
                    pats(pat, t, inside);
                }
            }
        }
        ExprKind::Match(_, arms) => {
            for a in arms {
                pats(&a.pat, t, inside);
            }
        }
        _ => {}
    });
}
