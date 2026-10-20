fn main() {
  let s = "quill";
  print_int(string_get(s, 0) + string_length(s));
  print_int(string_get(s, -1));
}
// expect: 118
// expect-error: string index -1 out of bounds (length 5)
