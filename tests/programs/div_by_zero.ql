fn divide(a, b) { a / b }
fn main() {
  println("before");
  print_int(divide(10, 0));
  println("after");
}
// expect: before
// expect-error: division by zero
