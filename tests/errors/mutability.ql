fn main() {
  let count = 0;
  count = count + 1;
  var total = 0;
  let add = |x| total + x;
  print_int(add(count));
}
// error: cannot assign to `count`: it is not mutable
// error: closures cannot capture the mutable variable `total`
