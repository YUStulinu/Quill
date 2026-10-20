//! The abstract syntax tree produced by the parser.

use crate::diag::Span;

/// Every expression, pattern and binding site gets a unique id, which the type checker uses to record
/// types and name resolutions in side tables.
pub type NodeId = u32;

#[derive(Clone, Debug)]
pub struct Ident {
    pub name: String,
    pub span: Span,
}

#[derive(Clone, Debug, Default)]
pub struct Program {
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Fn(FnDecl),
    Type(TypeDecl),
}

#[derive(Clone, Debug)]
pub struct FnDecl {
    pub name: Ident,
    pub params: Vec<Param>,
    pub ret: Option<TypeExpr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub id: NodeId,
    pub name: Ident,
    pub ty: Option<TypeExpr>,
}

#[derive(Clone, Debug)]
pub struct TypeDecl {
    pub name: Ident,
    pub params: Vec<Ident>,
    pub ctors: Vec<CtorDecl>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct CtorDecl {
    pub name: Ident,
    pub fields: Vec<TypeExpr>,
}

#[derive(Clone, Debug)]
pub enum TypeExpr {
    /// `Int`, `List[a]`
    Named(Ident, Vec<TypeExpr>),
    /// A lower-case name: a type variable.
    Var(Ident),
    /// `(Int, Bool) -> String`
    Fn(Vec<TypeExpr>, Box<TypeExpr>, Span),
    /// `(Int, Bool)`; `()` is Unit.
    Tuple(Vec<TypeExpr>, Span),
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(n, args) => args.last().map_or(n.span, |a| n.span.to(a.span())),
            TypeExpr::Var(n) => n.span,
            TypeExpr::Fn(_, _, s) | TypeExpr::Tuple(_, s) => *s,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    Concat,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::Concat => "++",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: NodeId,
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Bool(bool),
    Str(String),
    Unit,
    /// A variable, function, builtin or constructor name.
    Name(String),
    Call(Box<Expr>, Vec<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Block(Vec<Stmt>, Option<Box<Expr>>),
    Lambda(Vec<Param>, Box<Expr>),
    Match(Box<Expr>, Vec<Arm>),
    Tuple(Vec<Expr>),
    /// `(e)`: kept so that error messages and chained-comparison checks see the parentheses.
    Paren(Box<Expr>),
    Return(Option<Box<Expr>>),
    /// A parse error; type-checks as anything, so one mistake does not cascade.
    Error,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// `let pat = e;` or `var x = e;`
    Let {
        pat: Pattern,
        ty: Option<TypeExpr>,
        value: Expr,
        mutable: bool,
        span: Span,
    },
    /// `x = e;`
    Assign {
        target: Ident,
        target_id: NodeId,
        value: Expr,
        span: Span,
    },
    While {
        cond: Expr,
        body: Expr,
        span: Span,
    },
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Arm {
    pub pat: Pattern,
    pub guard: Option<Expr>,
    pub body: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub id: NodeId,
    pub kind: PatKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum PatKind {
    Wildcard,
    Bind(String),
    Int(i64),
    Bool(bool),
    Str(String),
    Unit,
    Tuple(Vec<Pattern>),
    Ctor(String, Vec<Pattern>),
}
