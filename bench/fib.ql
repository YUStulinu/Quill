// Recursive calls and integer arithmetic.
fn fib(n) { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }
fn main() { print_int(fib(32)); }
