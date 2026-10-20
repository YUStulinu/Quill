fn describe(n) {
  match n {
    _ => "anything",
    0 => "zero",
  }
}
fn main() { println(describe(0)); }
// warning: unreachable pattern
