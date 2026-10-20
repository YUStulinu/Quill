fn main() {
  let a = array_make(3, 7);
  print_int(array_get(a, 2));
  print_int(array_get(a, 3));
}
// expect: 7
// expect-error: array index 3 out of bounds (length 3)
