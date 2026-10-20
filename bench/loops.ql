// Tight loops over mutable variables: a sum with a modulo, and Collatz sequence lengths.
fn main() {
  var total = 0;
  var i = 0;
  while i < 30000000 {
    total = total + i % 7 * 3;
    i = i + 1;
  }
  print_int(total);
  var longest = 0;
  var best = 0;
  var n = 1;
  while n < 300000 {
    var x = n;
    var steps = 0;
    while x != 1 {
      if x % 2 == 0 { x = x / 2; } else { x = 3 * x + 1; }
      steps = steps + 1;
    }
    if steps > longest { longest = steps; best = n; }
    n = n + 1;
  }
  print_int(best);
}
