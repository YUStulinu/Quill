// Division and remainder by constants (compiled to multiplications at -O1 and above), over a range of
// positive and negative dividends, including the extremes of the 63-bit range.
fn check(n) {
  n / 3 + n % 3 + n / -3 + n % -3 + n / 7 + n % 7 + n / 10 + n % 10 + n / -10 + n / 641 + n % 641
    + n / 1000000007 + n % 1000000007 + n / 6 + n % -6 + n / 4611686018427387903 + n % 4611686018427387903
}
fn main() {
  var sum = 0;
  var n = -100000;
  while n <= 100000 {
    sum = sum + check(n);
    n = n + 7;
  }
  print_int(sum);
  print_int(check(4611686018427387903));
  print_int(check(-4611686018427387904));
  print_int(-4611686018427387904 / -4611686018427387904);
  print_int(-4611686018427387904 / 2 + -4611686018427387904 % 5);
  print_int(123456789 / 12345 * 12345 + 123456789 % 12345);
}
// expect: -135693
// expect: 1434621147478072330
// expect: -1434621147478072338
// expect: 1
// expect: -2305843009213693956
// expect: 123456789
