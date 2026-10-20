// String building and inspection.
fn digits_sum(s) {
  var total = 0;
  var i = 0;
  let n = string_length(s);
  while i < n { total = total + string_get(s, i) - 48; i = i + 1; }
  total
}

fn main() {
  var acc = 0;
  var i = 0;
  while i < 1000000 {
    let s = int_to_string(i) ++ "-" ++ int_to_string(i * 7);
    acc = acc + string_length(s) + digits_sum(int_to_string(i));
    i = i + 1;
  }
  print_int(acc);
}
