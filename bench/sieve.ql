// The sieve of Eratosthenes on an array, repeated.
fn sieve(n) {
  let composite = array_make(n + 1, false);
  var count = 0;
  var i = 2;
  while i <= n {
    if !array_get(composite, i) {
      count = count + 1;
      var j = i * i;
      while j <= n {
        array_set(composite, j, true);
        j = j + i;
      }
    }
    i = i + 1;
  }
  count
}

fn main() {
  var r = 0;
  var c = 0;
  while r < 10 { c = sieve(2000000); r = r + 1; }
  print_int(c);
}
