type Option[a] = None | Some(a)
fn unwrap(o) { match o { Some(x) => x, None => panic("unwrap of None") } }
fn main() {
  print_int(unwrap(Some(1)));
  print_int(unwrap(None));
}
// expect: 1
// expect-error: unwrap of None
