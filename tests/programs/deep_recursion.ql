// A recursion 200 000 calls deep: needs a large stack.
type List[a] = Nil | Cons(a, List[a])
fn build(n) { if n == 0 { Nil } else { Cons(n, build(n - 1)) } }
fn length(xs) { match xs { Nil => 0, Cons(_, rest) => 1 + length(rest) } }
fn main() { print_int(length(build(200000))); }
// expect: 200000
