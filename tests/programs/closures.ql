// Closures: captures, closures returning closures, functions and constructors as values.
type Option[a] = None | Some(a)

fn compose(f, g) { |x| f(g(x)) }
fn adder(n) { |x| x + n }
fn twice(f) { |x| f(f(x)) }
fn apply(f, x) { f(x) }
fn counter_from(start) {
  let step = 3;
  |times| start + step * times
}

fn main() {
  let add5 = adder(5);
  let double = |x| x * 2;
  let both = compose(add5, double);
  print_int(both(10));
  print_int(twice(twice(add5))(0));
  let c = counter_from(100);
  print_int(c(4));
  // A top-level function and a constructor used as values.
  print_int(apply(add5, 1));
  match apply(Some, 42) {
    Some(v) => print_int(v),
    None => println("none"),
  }
  // A builtin used as a value.
  let show = int_to_string;
  println(show(7) ++ "!");
  // Closures capturing closures.
  let base = 10;
  let f = |x| x + base;
  let g = |y| f(y) * f(y);
  print_int(g(1));
  // Let-polymorphism: one identity used at two types.
  let id = |x| x;
  println(id("poly"));
  print_int(id(3));
}
// expect: 25
// expect: 20
// expect: 112
// expect: 6
// expect: 42
// expect: 7!
// expect: 121
// expect: poly
// expect: 3
