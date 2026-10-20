fn main() {
  let x = 1 + "two";
  let y: String = 3;
  print_int(x);
  println(y);
}
// error: `+` needs Int operands, found String
// error: mismatched types: expected String, found Int
