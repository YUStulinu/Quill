type Shape = Circle(Int) | Square(Int) | Triangle(Int, Int)
type List[a] = Nil | Cons(a, List[a])

fn area(s) {
  match s {
    Circle(r) => r * r,
    Square(x) => x * x,
  }
}

fn second(xs) {
  match xs {
    Cons(_, Cons(y, _)) => y,
    Nil => 0,
  }
}

fn sign(b) { match b { true => 1 } }

fn main() {
  print_int(area(Circle(1)) + second(Nil) + sign(true));
}
// error: `Triangle(_, _)` is not handled
// error: `Cons(_, Nil)` is not handled
// error: `false` is not handled
