fn pick(b) { if b { 1 } else { "one" } }
fn only_then(b) { if b { 5 } }
fn arms(n) { match n { 0 => "zero", _ => 1 } }
fn main() {
  let _a = pick(true);
  let _b = only_then(false);
  let _c = arms(0);
}
// error: `if` and `else` have different types: Int and String
// error: `if` without `else` must produce (), but this produces Int
// error: match arms have different types: String and Int
