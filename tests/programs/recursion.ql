// Recursion, mutual recursion and a deep (non-tail) recursion.
fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }

fn is_even(n) { if n == 0 { true } else { is_odd(n - 1) } }
fn is_odd(n) { if n == 0 { false } else { is_even(n - 1) } }

fn sum_to(n) { if n == 0 { 0 } else { n + sum_to(n - 1) } }

fn ackermann(m, n) {
  if m == 0 { n + 1 }
  else if n == 0 { ackermann(m - 1, 1) }
  else { ackermann(m - 1, ackermann(m, n - 1)) }
}

fn main() {
  print_int(fib(25));
  println(if is_even(10) { "10 is even" } else { "10 is odd" });
  println(if is_odd(7) { "7 is odd" } else { "7 is even" });
  print_int(sum_to(100000));
  print_int(ackermann(2, 3));
}
// expect: 75025
// expect: 10 is even
// expect: 7 is odd
// expect: 5000050000
// expect: 9
