fn main() {
  let f = |x| x + 1;
  let g = |x| x + 1;
  if f == g { println("same") }
  print_int(4611686018427387904);
}
// error: functions cannot be compared with `==`
// error: integer literal is too large
