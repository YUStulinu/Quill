// Higher-order functions over lists: map, filter and fold with closures, many times.
type List[a] = Nil | Cons(a, List[a])

fn range(lo, hi, acc) { if hi < lo { acc } else { range(lo, hi - 1, Cons(hi, acc)) } }
fn map(xs, f) { match xs { Nil => Nil, Cons(x, r) => Cons(f(x), map(r, f)) } }
fn filter(xs, p) { match xs { Nil => Nil, Cons(x, r) => if p(x) { Cons(x, filter(r, p)) } else { filter(r, p) } } }
fn fold(xs, acc, f) { match xs { Nil => acc, Cons(x, r) => fold(r, f(acc, x), f) } }

fn main() {
  let xs = range(1, 100000, Nil);
  var total = 0;
  var round = 0;
  while round < 50 {
    let k = round;
    let ys = map(xs, |x| x * 3 + k);
    let zs = filter(ys, |y| y % 2 == 0);
    total = total + fold(zs, 0, |a, b| a + b % 1000);
    round = round + 1;
  }
  print_int(total);
}
