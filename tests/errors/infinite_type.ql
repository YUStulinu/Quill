fn self_apply(f) { f(f) }
fn main() { self_apply(|x| x); }
// error: infinite type
