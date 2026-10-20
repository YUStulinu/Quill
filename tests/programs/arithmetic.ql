// Integer arithmetic, including 63-bit wrap-around and truncating division.
fn main() {
  print_int(1 + 2 * 3);
  print_int((1 + 2) * 3);
  print_int(7 / 2);
  print_int(-7 / 2);
  print_int(7 % 3);
  print_int(-7 % 3);
  print_int(-(5 - 8));
  let big = 4611686018427387903;
  print_int(big);
  print_int(big + 1);
  print_int(-4611686018427387904 - 1);
  print_int(big * 2);
  print_int(123456789 * 987654321);
  println(int_to_string(10 / 3 * 3 + 10 % 3));
  if 3 < 4 && !(4 <= 3) || false { println("logic ok") } else { println("logic broken") }
  // Division and remainder by powers of two (compiled without idiv) round toward zero like the rest.
  let m7 = 0 - 7;
  let m9 = 0 - 9;
  print_int(m7 % 2);
  print_int((0 - 8) / 4);
  print_int(m9 / 4);
  print_int(m9 % 4);
  print_int(9 % 8);
  print_int((0 - 1) / 2);
  print_int((0 - 1) % 2);
  print_int(-4611686018427387904 / 2);
  print_int(m9 / 1024 + m9 % 1024);
  // The same with values the optimizer cannot know, so the shift sequences run at every level.
  let u9 = string_to_int("-9");
  let u7 = string_to_int("7");
  print_int(u9 / 4 * 100 + u9 % 4 * 10 + u9 / 2);
  print_int(u7 / 4 * 100 + u7 % 4 * 10 + u7 % 2);
  print_int(u9 * 3 - u7 * 1000);
  // Constants on the left of a comparison.
  if 5 > m7 { println("swap ok") } else { println("swap broken") }
}
// expect: 7
// expect: 9
// expect: 3
// expect: -3
// expect: 1
// expect: -1
// expect: 3
// expect: 4611686018427387903
// expect: -4611686018427387904
// expect: 4611686018427387903
// expect: -2
// expect: 121932631112635269
// expect: 10
// expect: logic ok
// expect: -1
// expect: -2
// expect: -2
// expect: -1
// expect: 1
// expect: 0
// expect: -1
// expect: -2305843009213693952
// expect: -9
// expect: -214
// expect: 131
// expect: -7027
// expect: swap ok
