// Allocates far more than fits in the first heap chunk, keeping some data alive throughout:
// the collector must free the garbage and never the live list.
type List[a] = Nil | Cons(a, List[a])

fn build(n, acc) { if n == 0 { acc } else { build(n - 1, Cons(n, acc)) } }
fn sum(xs) { match xs { Nil => 0, Cons(x, rest) => x + sum(rest) } }
fn length(xs) { match xs { Nil => 0, Cons(_, rest) => 1 + length(rest) } }

fn churn(rounds, keep) {
  var i = 0;
  var checksum = 0;
  while i < rounds {
    let tmp = build(1000, Nil);
    let label = "round " ++ int_to_string(i);
    checksum = checksum + sum(tmp) + string_length(label);
    i = i + 1;
  }
  checksum + sum(keep)
}

fn main() {
  let keep = build(10000, Nil);
  print_int(churn(2000, keep));
  print_int(length(keep));
  let pairs = build(50, Nil);
  let closures = array_make(100, |x| x);
  var j = 0;
  while j < 100 { let k = j; array_set(closures, j, |x| x + k); j = j + 1; }
  print_int(array_get(closures, 99)(1) + sum(pairs));
}
// expect: 1051023890
// expect: 10000
// expect: 1375
