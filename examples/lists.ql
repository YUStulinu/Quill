// Lists, higher-order functions and closures.

type List[a] = Nil | Cons(a, List[a])
type Option[a] = None | Some(a)

fn map(xs, f) {
  match xs {
    Nil => Nil,
    Cons(x, rest) => Cons(f(x), map(rest, f)),
  }
}

fn filter(xs, keep) {
  match xs {
    Nil => Nil,
    Cons(x, rest) if keep(x) => Cons(x, filter(rest, keep)),
    Cons(_, rest) => filter(rest, keep),
  }
}

fn fold(xs, acc, f) {
  match xs {
    Nil => acc,
    Cons(x, rest) => fold(rest, f(acc, x), f),
  }
}

fn range(lo, hi) {
  if lo >= hi { Nil } else { Cons(lo, range(lo + 1, hi)) }
}

fn find(xs, pred) {
  match xs {
    Nil => None,
    Cons(x, rest) => if pred(x) { Some(x) } else { find(rest, pred) },
  }
}

fn show_list(xs) {
  let body = fold(xs, "", |acc, x| if acc == "" { int_to_string(x) } else { acc ++ ", " ++ int_to_string(x) });
  "[" ++ body ++ "]"
}

fn main() {
  let xs = range(1, 11);
  let squares = map(xs, |x| x * x);
  let evens = filter(squares, |x| x % 2 == 0);
  println(show_list(evens));
  let total = fold(squares, 0, |a, b| a + b);
  println("sum of squares: " ++ int_to_string(total));
  let offset = 100;
  let shifted = map(xs, |x| x + offset);
  println(show_list(shifted));
  match find(xs, |x| x * x > 50) {
    Some(x) => println("first square above 50: " ++ int_to_string(x)),
    None => println("none"),
  }
}
