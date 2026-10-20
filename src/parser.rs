//! Recursive-descent parser (Pratt parsing for expressions) with error recovery: after a syntax error it
//! skips to the next statement or item, so that one mistake reports one error and the language server can
//! still type-check the rest of the file.

use crate::ast::*;
use crate::diag::{Diagnostic, Diagnostics, Span};
use crate::lexer::{Tok, Token, lex};

pub struct Parser<'a> {
    toks: Vec<Token>,
    pos: usize,
    next_id: NodeId,
    diags: &'a mut Diagnostics,
}

struct ParseError;
type PResult<T> = Result<T, ParseError>;

/// Parses a whole program. Errors go to `diags`; the returned program contains everything that parsed.
pub fn parse_program(src: &str, diags: &mut Diagnostics) -> Program {
    let toks = lex(src, diags);
    let mut p = Parser { toks, pos: 0, next_id: 0, diags };
    p.program()
}

/// Parses with node ids starting at `first_id` (used by the REPL, which parses pieces separately).
pub fn parse_program_from(src: &str, first_id: NodeId, diags: &mut Diagnostics) -> (Program, NodeId) {
    let toks = lex(src, diags);
    let mut p = Parser { toks, pos: 0, next_id: first_id, diags };
    let prog = p.program();
    (prog, p.next_id)
}

impl<'a> Parser<'a> {
    fn id(&mut self) -> NodeId {
        self.next_id += 1;
        self.next_id
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn peek_at(&self, n: usize) -> &Tok {
        &self.toks[(self.pos + n).min(self.toks.len() - 1)].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn prev_span(&self) -> Span {
        self.toks[self.pos.saturating_sub(1)].span
    }

    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn at(&self, t: &Tok) -> bool {
        self.peek() == t
    }

    fn eat(&mut self, t: &Tok) -> bool {
        if self.at(t) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn error_here(&mut self, expected: &str) -> ParseError {
        let found = self.peek().describe();
        let span = self.span();
        self.diags.push(Diagnostic::error(span, format!("expected {expected}, found {found}")).with_label(format!("expected {expected}")));
        ParseError
    }

    fn expect(&mut self, t: &Tok) -> PResult<Span> {
        if self.at(t) { Ok(self.bump().span) } else { Err(self.error_here(&format!("`{}`", t.text()))) }
    }

    fn ident(&mut self) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::Ident(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            _ => Err(self.error_here("a name")),
        }
    }

    fn uident(&mut self) -> PResult<Ident> {
        match self.peek().clone() {
            Tok::UIdent(name) => {
                let span = self.bump().span;
                Ok(Ident { name, span })
            }
            _ => Err(self.error_here("a capitalized name")),
        }
    }

    // ------------------------------------------------------------------
    // Items
    // ------------------------------------------------------------------

    fn program(&mut self) -> Program {
        let mut items = Vec::new();
        while !self.at(&Tok::Eof) {
            let start = self.pos;
            match self.item() {
                Ok(item) => items.push(item),
                Err(ParseError) => {
                    // Skip to the next item.
                    if self.pos == start {
                        self.bump();
                    }
                    while !matches!(self.peek(), Tok::Fn | Tok::Type | Tok::Eof) {
                        self.bump();
                    }
                }
            }
        }
        Program { items }
    }

    fn item(&mut self) -> PResult<Item> {
        match self.peek() {
            Tok::Fn => self.fn_decl().map(Item::Fn),
            Tok::Type => self.type_decl().map(Item::Type),
            _ => Err(self.error_here("`fn` or `type`")),
        }
    }

    fn fn_decl(&mut self) -> PResult<FnDecl> {
        let start = self.expect(&Tok::Fn)?;
        let name = self.ident()?;
        self.expect(&Tok::LParen)?;
        let params = self.params(&Tok::RParen)?;
        self.expect(&Tok::RParen)?;
        let ret = if self.eat(&Tok::Arrow) { Some(self.type_expr()?) } else { None };
        if !self.at(&Tok::LBrace) {
            return Err(self.error_here("`{` to start the function body"));
        }
        let body = self.block()?;
        let span = start.to(body.span);
        Ok(FnDecl { name, params, ret, body, span })
    }

    fn params(&mut self, close: &Tok) -> PResult<Vec<Param>> {
        let mut params = Vec::new();
        while !self.at(close) {
            let name = match self.peek() {
                Tok::Underscore => {
                    let span = self.bump().span;
                    Ident { name: "_".into(), span }
                }
                _ => self.ident()?,
            };
            let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
            let id = self.id();
            params.push(Param { id, name, ty });
            if !self.eat(&Tok::Comma) {
                break;
            }
        }
        Ok(params)
    }

    fn type_decl(&mut self) -> PResult<TypeDecl> {
        let start = self.expect(&Tok::Type)?;
        let name = self.uident()?;
        let mut params = Vec::new();
        if self.eat(&Tok::LBracket) {
            while !self.at(&Tok::RBracket) {
                params.push(self.ident()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RBracket)?;
        }
        self.expect(&Tok::Eq)?;
        self.eat(&Tok::Pipe);
        let mut ctors = Vec::new();
        loop {
            let cname = self.uident()?;
            let mut fields = Vec::new();
            if self.eat(&Tok::LParen) {
                while !self.at(&Tok::RParen) {
                    fields.push(self.type_expr()?);
                    if !self.eat(&Tok::Comma) {
                        break;
                    }
                }
                self.expect(&Tok::RParen)?;
            }
            ctors.push(CtorDecl { name: cname, fields });
            if !self.eat(&Tok::Pipe) {
                break;
            }
        }
        let span = start.to(self.prev_span());
        Ok(TypeDecl { name, params, ctors, span })
    }

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        let atom = if self.eat(&Tok::LParen) {
            let mut elems = Vec::new();
            while !self.at(&Tok::RParen) {
                elems.push(self.type_expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            self.expect(&Tok::RParen)?;
            let span = start.to(self.prev_span());
            if self.at(&Tok::Arrow) {
                self.bump();
                let ret = self.type_expr()?;
                let span = span.to(ret.span());
                return Ok(TypeExpr::Fn(elems, Box::new(ret), span));
            }
            if elems.len() == 1 { elems.pop().unwrap() } else { TypeExpr::Tuple(elems, span) }
        } else {
            match self.peek().clone() {
                Tok::UIdent(_) => {
                    let name = self.uident()?;
                    let mut args = Vec::new();
                    if self.eat(&Tok::LBracket) {
                        while !self.at(&Tok::RBracket) {
                            args.push(self.type_expr()?);
                            if !self.eat(&Tok::Comma) {
                                break;
                            }
                        }
                        self.expect(&Tok::RBracket)?;
                    }
                    TypeExpr::Named(name, args)
                }
                Tok::Ident(_) => TypeExpr::Var(self.ident()?),
                _ => return Err(self.error_here("a type")),
            }
        };
        if self.eat(&Tok::Arrow) {
            let ret = self.type_expr()?;
            let span = atom.span().to(ret.span());
            return Ok(TypeExpr::Fn(vec![atom], Box::new(ret), span));
        }
        Ok(atom)
    }

    // ------------------------------------------------------------------
    // Blocks and statements
    // ------------------------------------------------------------------

    fn block(&mut self) -> PResult<Expr> {
        let start = self.expect(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        let mut tail: Option<Box<Expr>> = None;
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            if let Some(t) = tail.take() {
                // An expression without `;` that is followed by more statements: only block-like
                // expressions may do that.
                if is_block_like(&t) {
                    stmts.push(Stmt::Expr(*t));
                } else {
                    let span = self.span();
                    self.diags.push(
                        Diagnostic::error(span, format!("expected `;` or `}}`, found {}", self.peek().describe()))
                            .with_label("expected `;` before this"),
                    );
                    stmts.push(Stmt::Expr(*t));
                }
            }
            let before = self.pos;
            match self.stmt() {
                Ok(StmtOrTail::Stmt(s)) => stmts.push(s),
                Ok(StmtOrTail::Tail(e)) => tail = Some(Box::new(e)),
                Err(ParseError) => {
                    // Skip to the end of this statement.
                    if self.pos == before {
                        self.bump();
                    }
                    let mut depth = 0;
                    loop {
                        match self.peek() {
                            Tok::Eof => break,
                            Tok::LBrace => depth += 1,
                            Tok::RBrace if depth == 0 => break,
                            Tok::RBrace => depth -= 1,
                            Tok::Semi if depth == 0 => {
                                self.bump();
                                break;
                            }
                            Tok::Fn | Tok::Type if depth == 0 => break,
                            _ => {}
                        }
                        self.bump();
                    }
                    if matches!(self.peek(), Tok::Fn | Tok::Type) {
                        break;
                    }
                }
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        let id = self.id();
        Ok(Expr { id, kind: ExprKind::Block(stmts, tail), span: start.to(end) })
    }

    fn stmt(&mut self) -> PResult<StmtOrTail> {
        let start = self.span();
        match self.peek().clone() {
            Tok::Let => {
                self.bump();
                let pat = self.pattern()?;
                let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                self.expect(&Tok::Eq)?;
                let value = self.expr()?;
                let end = self.expect(&Tok::Semi)?;
                Ok(StmtOrTail::Stmt(Stmt::Let { pat, ty, value, mutable: false, span: start.to(end) }))
            }
            Tok::Var => {
                self.bump();
                let name = self.ident()?;
                let id = self.id();
                let pat = Pattern { id, span: name.span, kind: PatKind::Bind(name.name) };
                let ty = if self.eat(&Tok::Colon) { Some(self.type_expr()?) } else { None };
                self.expect(&Tok::Eq)?;
                let value = self.expr()?;
                let end = self.expect(&Tok::Semi)?;
                Ok(StmtOrTail::Stmt(Stmt::Let { pat, ty, value, mutable: true, span: start.to(end) }))
            }
            Tok::While => {
                self.bump();
                let cond = self.expr()?;
                let body = self.block()?;
                let span = start.to(body.span);
                self.eat(&Tok::Semi);
                Ok(StmtOrTail::Stmt(Stmt::While { cond, body, span }))
            }
            Tok::Ident(_) if self.peek_at(1) == &Tok::Eq => {
                let target = self.ident()?;
                self.bump(); // =
                let value = self.expr()?;
                let end = self.expect(&Tok::Semi)?;
                let target_id = self.id();
                Ok(StmtOrTail::Stmt(Stmt::Assign { target, target_id, value, span: start.to(end) }))
            }
            // A statement that starts with `if`, `match` or a block ends with its closing brace (as in
            // Rust): `if c { a } else { b }` followed by `(x)` on the next line is not a call.
            Tok::If | Tok::Match | Tok::LBrace => {
                let e = match self.peek() {
                    Tok::If => self.if_expr()?,
                    Tok::Match => self.match_expr()?,
                    _ => self.block()?,
                };
                if self.eat(&Tok::Semi) { Ok(StmtOrTail::Stmt(Stmt::Expr(e))) } else { Ok(StmtOrTail::Tail(e)) }
            }
            _ => {
                let e = self.expr()?;
                if self.eat(&Tok::Semi) {
                    Ok(StmtOrTail::Stmt(Stmt::Expr(e)))
                } else if self.at(&Tok::RBrace) || is_block_like(&e) {
                    Ok(StmtOrTail::Tail(e))
                } else {
                    Err(self.error_here("`;` or `}`"))
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Expressions
    // ------------------------------------------------------------------

    fn expr(&mut self) -> PResult<Expr> {
        self.binary(0)
    }

    fn binary(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.unary()?;
        loop {
            let (op, prec, right_assoc) = match self.peek() {
                Tok::OrOr => (BinOp::Or, 1, false),
                Tok::AndAnd => (BinOp::And, 2, false),
                Tok::EqEq => (BinOp::Eq, 3, false),
                Tok::Ne => (BinOp::Ne, 3, false),
                Tok::Lt => (BinOp::Lt, 3, false),
                Tok::Le => (BinOp::Le, 3, false),
                Tok::Gt => (BinOp::Gt, 3, false),
                Tok::Ge => (BinOp::Ge, 3, false),
                Tok::PlusPlus => (BinOp::Concat, 4, true),
                Tok::Plus => (BinOp::Add, 5, false),
                Tok::Minus => (BinOp::Sub, 5, false),
                Tok::Star => (BinOp::Mul, 6, false),
                Tok::Slash => (BinOp::Div, 6, false),
                Tok::Percent => (BinOp::Mod, 6, false),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            self.bump();
            let next = if right_assoc { prec } else { prec + 1 };
            let rhs = self.binary(next)?;
            if prec == 3
                && let ExprKind::Binary(prev, _, _) = &lhs.kind
                && matches!(prev, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
                && lhs_is_unparenthesized(&lhs)
            {
                self.diags.push(
                    Diagnostic::error(lhs.span.to(rhs.span), "comparison operators cannot be chained")
                        .with_note("use `&&`: `a < b && b < c`"),
                );
            }
            let span = lhs.span.to(rhs.span);
            let id = self.id();
            lhs = Expr { id, kind: ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)), span };
        }
        Ok(lhs)
    }

    fn unary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let op = match self.peek() {
            Tok::Minus => Some(UnOp::Neg),
            Tok::Bang => Some(UnOp::Not),
            _ => None,
        };
        if let (Some(UnOp::Neg), Tok::Int(n)) = (op, self.peek_at(1).clone()) {
            // A negative literal: the most negative integer is only representable this way.
            self.bump();
            self.bump();
            let span = start.to(self.prev_span());
            let id = self.id();
            let e = Expr { id, kind: ExprKind::Int(-n), span };
            return self.postfix_from(e);
        }
        if let Some(op) = op {
            self.bump();
            let e = self.unary()?;
            let span = start.to(e.span);
            // Fold `-<literal>` so that the most negative literal is representable.
            if let (UnOp::Neg, ExprKind::Int(n)) = (op, &e.kind) {
                // Wrapped to 63 bits like every integer operation: -(-2^62) is -2^62.
                let id = self.id();
                return Ok(Expr { id, kind: ExprKind::Int(crate::interp::wrap63(n.wrapping_neg())), span });
            }
            let id = self.id();
            return Ok(Expr { id, kind: ExprKind::Unary(op, Box::new(e)), span });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let e = self.primary()?;
        self.postfix_from(e)
    }

    fn postfix_from(&mut self, mut e: Expr) -> PResult<Expr> {
        while self.at(&Tok::LParen) {
            self.bump();
            let mut args = Vec::new();
            while !self.at(&Tok::RParen) {
                args.push(self.expr()?);
                if !self.eat(&Tok::Comma) {
                    break;
                }
            }
            let end = self.expect(&Tok::RParen)?;
            let span = e.span.to(end);
            let id = self.id();
            e = Expr { id, kind: ExprKind::Call(Box::new(e), args), span };
        }
        Ok(e)
    }

    fn primary(&mut self) -> PResult<Expr> {
        let start = self.span();
        let kind = match self.peek().clone() {
            Tok::Int(n) => {
                if n > crate::lexer::INT_MAX {
                    let span = self.span();
                    self.diags.push(
                        Diagnostic::error(span, "integer literal is too large")
                            .with_note(format!("Quill integers are 63-bit: the largest is {}", crate::lexer::INT_MAX)),
                    );
                }
                self.bump();
                ExprKind::Int(n)
            }
            Tok::Str(s) => {
                self.bump();
                ExprKind::Str(s)
            }
            Tok::True => {
                self.bump();
                ExprKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                ExprKind::Bool(false)
            }
            Tok::Ident(name) | Tok::UIdent(name) => {
                self.bump();
                ExprKind::Name(name)
            }
            Tok::LParen => {
                self.bump();
                if self.eat(&Tok::RParen) {
                    ExprKind::Unit
                } else {
                    let first = self.expr()?;
                    if self.eat(&Tok::RParen) {
                        let span = start.to(self.prev_span());
                        let id = self.id();
                        return Ok(Expr { id, kind: ExprKind::Paren(Box::new(first)), span });
                    }
                    let mut elems = vec![first];
                    while self.eat(&Tok::Comma) {
                        if self.at(&Tok::RParen) {
                            break;
                        }
                        elems.push(self.expr()?);
                    }
                    self.expect(&Tok::RParen)?;
                    ExprKind::Tuple(elems)
                }
            }
            Tok::LBrace => return self.block(),
            Tok::If => return self.if_expr(),
            Tok::Match => return self.match_expr(),
            Tok::Pipe | Tok::OrOr => return self.lambda(),
            Tok::Return => {
                self.bump();
                let value = if matches!(self.peek(), Tok::Semi | Tok::RBrace | Tok::Comma | Tok::RParen) {
                    None
                } else {
                    Some(Box::new(self.expr()?))
                };
                ExprKind::Return(value)
            }
            Tok::Fn => {
                let span = self.span();
                self.diags.push(
                    Diagnostic::error(span, "functions cannot be declared inside other functions")
                        .with_note("use a closure instead: `let f = |x| x + 1;`"),
                );
                return Err(ParseError);
            }
            _ => return Err(self.error_here("an expression")),
        };
        let span = start.to(self.prev_span());
        let id = self.id();
        Ok(Expr { id, kind, span })
    }

    fn if_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&Tok::If)?;
        let cond = self.expr()?;
        if !self.at(&Tok::LBrace) {
            return Err(self.error_here("`{` after the condition"));
        }
        let then = self.block()?;
        let els = if self.eat(&Tok::Else) {
            if self.at(&Tok::If) { Some(Box::new(self.if_expr()?)) } else { Some(Box::new(self.block()?)) }
        } else {
            None
        };
        let span = start.to(els.as_ref().map_or(then.span, |e| e.span));
        let id = self.id();
        Ok(Expr { id, kind: ExprKind::If(Box::new(cond), Box::new(then), els), span })
    }

    fn match_expr(&mut self) -> PResult<Expr> {
        let start = self.expect(&Tok::Match)?;
        let scrutinee = self.expr()?;
        self.expect(&Tok::LBrace)?;
        let mut arms = Vec::new();
        while !self.at(&Tok::RBrace) && !self.at(&Tok::Eof) {
            let pat = self.pattern()?;
            let guard = if self.eat(&Tok::If) { Some(self.expr()?) } else { None };
            self.expect(&Tok::FatArrow)?;
            let body = self.expr()?;
            let span = pat.span.to(body.span);
            let block_like = is_block_like(&body);
            arms.push(Arm { pat, guard, body, span });
            if !self.eat(&Tok::Comma) && !block_like && !self.at(&Tok::RBrace) {
                return Err(self.error_here("`,` between match arms"));
            }
        }
        let end = self.expect(&Tok::RBrace)?;
        let id = self.id();
        Ok(Expr { id, kind: ExprKind::Match(Box::new(scrutinee), arms), span: start.to(end) })
    }

    fn lambda(&mut self) -> PResult<Expr> {
        let start = self.span();
        let params = if self.eat(&Tok::OrOr) {
            vec![]
        } else {
            self.expect(&Tok::Pipe)?;
            let ps = self.params(&Tok::Pipe)?;
            self.expect(&Tok::Pipe)?;
            ps
        };
        let body = self.expr()?;
        let span = start.to(body.span);
        let id = self.id();
        Ok(Expr { id, kind: ExprKind::Lambda(params, Box::new(body)), span })
    }

    // ------------------------------------------------------------------
    // Patterns
    // ------------------------------------------------------------------

    fn pattern(&mut self) -> PResult<Pattern> {
        let start = self.span();
        let kind = match self.peek().clone() {
            Tok::Underscore => {
                self.bump();
                PatKind::Wildcard
            }
            Tok::Ident(name) => {
                self.bump();
                PatKind::Bind(name)
            }
            Tok::Int(n) => {
                self.bump();
                PatKind::Int(n)
            }
            Tok::Minus => {
                self.bump();
                match self.peek().clone() {
                    Tok::Int(n) => {
                        self.bump();
                        PatKind::Int(-n)
                    }
                    _ => return Err(self.error_here("an integer")),
                }
            }
            Tok::Str(s) => {
                self.bump();
                PatKind::Str(s)
            }
            Tok::True => {
                self.bump();
                PatKind::Bool(true)
            }
            Tok::False => {
                self.bump();
                PatKind::Bool(false)
            }
            Tok::LParen => {
                self.bump();
                if self.eat(&Tok::RParen) {
                    PatKind::Unit
                } else {
                    let mut elems = vec![self.pattern()?];
                    while self.eat(&Tok::Comma) {
                        if self.at(&Tok::RParen) {
                            break;
                        }
                        elems.push(self.pattern()?);
                    }
                    self.expect(&Tok::RParen)?;
                    if elems.len() == 1 {
                        return Ok(elems.pop().unwrap());
                    }
                    PatKind::Tuple(elems)
                }
            }
            Tok::UIdent(name) => {
                self.bump();
                let mut args = Vec::new();
                if self.eat(&Tok::LParen) {
                    while !self.at(&Tok::RParen) {
                        args.push(self.pattern()?);
                        if !self.eat(&Tok::Comma) {
                            break;
                        }
                    }
                    self.expect(&Tok::RParen)?;
                }
                PatKind::Ctor(name, args)
            }
            _ => return Err(self.error_here("a pattern")),
        };
        let span = start.to(self.prev_span());
        let id = self.id();
        Ok(Pattern { id, kind, span })
    }
}

enum StmtOrTail {
    Stmt(Stmt),
    Tail(Expr),
}

fn is_block_like(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::If(..) | ExprKind::Match(..) | ExprKind::Block(..))
}

fn lhs_is_unparenthesized(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::Binary(..))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> Program {
        let mut d = Diagnostics::default();
        let p = parse_program(src, &mut d);
        assert!(!d.has_errors(), "{:#?}", d.list);
        p
    }

    #[test]
    fn parses_functions_types_and_expressions() {
        let p = parse_ok(
            "type List[a] = Nil | Cons(a, List[a])
             fn map(xs: List[a], f: (a) -> b) -> List[b] {
               match xs { Nil => Nil, Cons(x, rest) => Cons(f(x), map(rest, f)) }
             }
             fn main() { let ys = map(Cons(1, Nil), |x| x * 2 + 1); var i = 0; while i < 3 { i = i + 1; } println(\"ok\") }",
        );
        assert_eq!(p.items.len(), 3);
    }

    #[test]
    fn precedence_is_conventional() {
        let p = parse_ok("fn f() { 1 + 2 * 3 == 7 && true || false }");
        let Item::Fn(f) = &p.items[0] else { panic!() };
        let ExprKind::Block(_, Some(tail)) = &f.body.kind else { panic!() };
        assert!(matches!(tail.kind, ExprKind::Binary(BinOp::Or, _, _)));
    }

    #[test]
    fn multiplication_binds_tighter_than_addition() {
        let p = parse_ok("fn f(a, b, c) { a + b * c - a - b }");
        let Item::Fn(f) = &p.items[0] else { panic!() };
        let ExprKind::Block(_, Some(tail)) = &f.body.kind else { panic!() };
        // ((a + (b * c)) - a) - b
        let ExprKind::Binary(BinOp::Sub, l, _) = &tail.kind else { panic!("{:?}", tail.kind) };
        let ExprKind::Binary(BinOp::Sub, l2, _) = &l.kind else { panic!() };
        let ExprKind::Binary(BinOp::Add, _, r) = &l2.kind else { panic!() };
        assert!(matches!(r.kind, ExprKind::Binary(BinOp::Mul, _, _)));
    }

    #[test]
    fn concatenation_is_right_associative() {
        let p = parse_ok("fn f(a, b, c) { a ++ b ++ c }");
        let Item::Fn(f) = &p.items[0] else { panic!() };
        let ExprKind::Block(_, Some(tail)) = &f.body.kind else { panic!() };
        let ExprKind::Binary(BinOp::Concat, _, r) = &tail.kind else { panic!() };
        assert!(matches!(r.kind, ExprKind::Binary(BinOp::Concat, _, _)));
    }

    #[test]
    fn block_like_statements_end_at_their_brace() {
        let p = parse_ok(
            "fn f(x) { if x { print_int(1) } else { print_int(2) }
(x + 1) }",
        );
        let Item::Fn(f) = &p.items[0] else { panic!() };
        let ExprKind::Block(stmts, Some(tail)) = &f.body.kind else { panic!() };
        assert_eq!(stmts.len(), 1);
        assert!(matches!(tail.kind, ExprKind::Paren(_)));
    }

    #[test]
    fn recovers_after_errors() {
        let mut d = Diagnostics::default();
        let p = parse_program("fn a() { let = 1; 2 } fn b() { 3 }", &mut d);
        assert_eq!(d.list.len(), 1);
        assert_eq!(p.items.len(), 2);
    }

    #[test]
    fn rejects_chained_comparisons() {
        let mut d = Diagnostics::default();
        parse_program("fn f(a, b, c) { a < b < c }", &mut d);
        assert!(d.list[0].message.contains("chained"));
    }
}
