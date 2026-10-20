type Option[a] = None | Some(a)
fn add(a, b) { a + b }
fn main() {
  print_int(add(1));
  print_int(lenght("abc"));
  let o = Some(1, 2);
  match o { Some(x, y) => print_int(x), None => () }
  print_int(prnt_int);
}
// error: `add` takes 2 arguments, but 1 was given
// error: cannot find `lenght` in this scope
// error: `Some` takes 1 field, but 2 were given
// error: constructor `Some` has 1 field, but the pattern has 2
// error: cannot find `prnt_int` in this scope
