// A calculator: a tokenizer, a recursive-descent parser and an evaluator, all in Quill.
// Errors are values (a Result type) rather than crashes.

type Token = Num(Int) | Plus | Minus | Times | Divide | LParen | RParen | End
type List[a] = Nil | Cons(a, List[a])
type Expr = Lit(Int) | Add(Expr, Expr) | Sub(Expr, Expr) | Mul(Expr, Expr) | Div(Expr, Expr) | Neg(Expr)
type Result[a] = Ok(a) | Err(String)

fn is_digit(c) { c >= 48 && c <= 57 }

// Turns "12 + 3*(4-1)" into a list of tokens.
fn tokenize(s, i) {
  if i >= string_length(s) {
    Ok(Cons(End, Nil))
  } else {
    let c = string_get(s, i);
    if c == 32 {
      tokenize(s, i + 1)
    } else if is_digit(c) {
      var j = i;
      var n = 0;
      while j < string_length(s) && is_digit(string_get(s, j)) {
        n = n * 10 + string_get(s, j) - 48;
        j = j + 1;
      }
      prepend(Num(n), tokenize(s, j))
    } else {
      let t = match c {
        43 => Ok(Plus),
        45 => Ok(Minus),
        42 => Ok(Times),
        47 => Ok(Divide),
        40 => Ok(LParen),
        41 => Ok(RParen),
        _ => Err("unexpected character '" ++ char_to_string(c) ++ "' at " ++ int_to_string(i)),
      };
      match t {
        Ok(tok) => prepend(tok, tokenize(s, i + 1)),
        Err(e) => Err(e),
      }
    }
  }
}

fn prepend(t, rest) {
  match rest {
    Ok(ts) => Ok(Cons(t, ts)),
    Err(e) => Err(e),
  }
}

// Grammar:  expr = term (('+' | '-') term)*    term = factor (('*' | '/') factor)*
//           factor = number | '-' factor | '(' expr ')'
// Each parser returns the expression and the remaining tokens.
fn parse_expr(ts) {
  match parse_term(ts) {
    Ok((lhs, rest)) => expr_tail(lhs, rest),
    Err(e) => Err(e),
  }
}

fn expr_tail(lhs, ts) {
  match ts {
    Cons(Plus, rest) => match parse_term(rest) {
      Ok((rhs, rest2)) => expr_tail(Add(lhs, rhs), rest2),
      Err(e) => Err(e),
    },
    Cons(Minus, rest) => match parse_term(rest) {
      Ok((rhs, rest2)) => expr_tail(Sub(lhs, rhs), rest2),
      Err(e) => Err(e),
    },
    _ => Ok((lhs, ts)),
  }
}

fn parse_term(ts) {
  match parse_factor(ts) {
    Ok((lhs, rest)) => term_tail(lhs, rest),
    Err(e) => Err(e),
  }
}

fn term_tail(lhs, ts) {
  match ts {
    Cons(Times, rest) => match parse_factor(rest) {
      Ok((rhs, rest2)) => term_tail(Mul(lhs, rhs), rest2),
      Err(e) => Err(e),
    },
    Cons(Divide, rest) => match parse_factor(rest) {
      Ok((rhs, rest2)) => term_tail(Div(lhs, rhs), rest2),
      Err(e) => Err(e),
    },
    _ => Ok((lhs, ts)),
  }
}

fn parse_factor(ts) {
  match ts {
    Cons(Num(n), rest) => Ok((Lit(n), rest)),
    Cons(Minus, rest) => match parse_factor(rest) {
      Ok((e, rest2)) => Ok((Neg(e), rest2)),
      Err(err) => Err(err),
    },
    Cons(LParen, rest) => match parse_expr(rest) {
      Ok((e, Cons(RParen, rest2))) => Ok((e, rest2)),
      Ok(_) => Err("expected ')'"),
      Err(err) => Err(err),
    },
    _ => Err("expected a number or '('"),
  }
}

fn eval(e) {
  match e {
    Lit(n) => Ok(n),
    Neg(a) => map_result(eval(a), |x| 0 - x),
    Add(a, b) => both(eval(a), eval(b), |x, y| Ok(x + y)),
    Sub(a, b) => both(eval(a), eval(b), |x, y| Ok(x - y)),
    Mul(a, b) => both(eval(a), eval(b), |x, y| Ok(x * y)),
    Div(a, b) => both(eval(a), eval(b), |x, y| if y == 0 { Err("division by zero") } else { Ok(x / y) }),
  }
}

fn map_result(r, f) { match r { Ok(x) => Ok(f(x)), Err(e) => Err(e) } }

fn both(ra, rb, f) {
  match (ra, rb) {
    (Ok(a), Ok(b)) => f(a, b),
    (Err(e), _) => Err(e),
    (_, Err(e)) => Err(e),
  }
}

fn calculate(s) {
  let parsed = match tokenize(s, 0) {
    Ok(ts) => match parse_expr(ts) {
      Ok((e, Cons(End, Nil))) => Ok(e),
      Ok(_) => Err("unexpected input after the expression"),
      Err(e) => Err(e),
    },
    Err(e) => Err(e),
  };
  match parsed {
    Ok(e) => eval(e),
    Err(e) => Err(e),
  }
}

fn show(input) {
  match calculate(input) {
    Ok(n) => println(input ++ " = " ++ int_to_string(n)),
    Err(e) => println(input ++ " -> error: " ++ e),
  }
}

fn main() {
  show("1 + 2 * 3");
  show("(1 + 2) * 3");
  show("2 * (3 + 4) * 5 - 10 / 2");
  show("-(4 - 10) * -2");
  show("100 / (5 - 5)");
  show("3 + * 4");
  show("7 $ 2");
}
