// Mutable variables and while loops (SSA construction through loop headers).
fn collatz_steps(start) {
  var n = start;
  var steps = 0;
  while n != 1 {
    if n % 2 == 0 { n = n / 2; } else { n = 3 * n + 1; }
    steps = steps + 1;
  }
  steps
}

fn primes_below(limit) {
  var count = 0;
  var i = 2;
  while i < limit {
    var j = 2;
    var prime = true;
    while j * j <= i && prime {
      if i % j == 0 { prime = false; }
      j = j + 1;
    }
    if prime { count = count + 1; }
    i = i + 1;
  }
  count
}

fn early_return(limit) {
  var i = 0;
  while true {
    if i * i > limit { return i; }
    i = i + 1;
  }
  -1
}

fn main() {
  print_int(collatz_steps(27));
  print_int(primes_below(1000));
  print_int(early_return(50));
  var a = 1;
  var b = 1;
  var k = 0;
  while k < 10 { let t = a + b; a = b; b = t; k = k + 1; }
  print_int(b);
}
// expect: 111
// expect: 168
// expect: 8
// expect: 144
