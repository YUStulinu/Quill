// Pattern matching: nested constructors, literals, strings, tuples, guards, wildcards.
type Expr = Num(Int) | Add(Expr, Expr) | Mul(Expr, Expr) | Neg(Expr)
type Color = Red | Green | Blue

fn eval(e) {
  match e {
    Num(n) => n,
    Add(a, b) => eval(a) + eval(b),
    Mul(Num(0), _) => 0,
    Mul(a, b) => eval(a) * eval(b),
    Neg(Neg(x)) => eval(x),
    Neg(x) => -eval(x),
  }
}

fn describe(n) {
  match n {
    0 => "zero",
    1 => "one",
    x if x < 0 => "negative",
    x if x % 2 == 0 => "even",
    _ => "odd",
  }
}

fn color_name(c) { match c { Red => "red", Green => "green", Blue => "blue" } }

fn greet(name) {
  match name {
    "world" => "hello, world",
    "" => "hello, nobody",
    other => "hi " ++ other,
  }
}

fn classify(p) {
  match p {
    (0, 0) => "origin",
    (0, _) => "on the y axis",
    (_, 0) => "on the x axis",
    (x, y) if x == y => "on the diagonal",
    _ => "somewhere",
  }
}

fn main() {
  let e = Add(Num(2), Mul(Num(3), Neg(Neg(Num(4)))));
  print_int(eval(e));
  print_int(eval(Mul(Num(0), Num(99))));
  println(describe(0) ++ " " ++ describe(1) ++ " " ++ describe(-5) ++ " " ++ describe(8) ++ " " ++ describe(9));
  println(color_name(Green));
  println(greet("world"));
  println(greet(""));
  println(greet("Ana"));
  println(classify((0, 0)));
  println(classify((0, 3)));
  println(classify((4, 0)));
  println(classify((5, 5)));
  println(classify((1, 2)));
  let (a, b) = (10, 20);
  print_int(a + b);
}
// expect: 14
// expect: 0
// expect: zero one negative even odd
// expect: green
// expect: hello, world
// expect: hello, nobody
// expect: hi Ana
// expect: origin
// expect: on the y axis
// expect: on the x axis
// expect: on the diagonal
// expect: somewhere
// expect: 30
