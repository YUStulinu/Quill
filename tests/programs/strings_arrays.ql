// Strings, arrays and structural equality.
type Tree = Leaf | Node(Tree, Int, Tree)

fn insert(t, x) {
  match t {
    Leaf => Node(Leaf, x, Leaf),
    Node(l, v, r) => if x < v { Node(insert(l, x), v, r) } else { Node(l, v, insert(r, x)) },
  }
}

fn reverse_string(s) {
  var out = "";
  var i = string_length(s) - 1;
  while i >= 0 { out = out ++ char_to_string(string_get(s, i)); i = i - 1; }
  out
}

fn main() {
  let s = "Quill";
  print_int(string_length(s));
  println(reverse_string(s));
  println(substring("compiler", 3, 4));
  print_int(string_to_int("-1234") + 1);
  let arr = array_make(10, 0);
  var i = 0;
  while i < array_length(arr) { array_set(arr, i, i * i); i = i + 1; }
  var total = 0;
  i = 0;
  while i < 10 { total = total + array_get(arr, i); i = i + 1; }
  print_int(total);
  let t1 = insert(insert(insert(Leaf, 5), 2), 8);
  let t2 = insert(insert(insert(Leaf, 5), 8), 2);
  let t3 = insert(insert(insert(Leaf, 2), 5), 8);
  println(if t1 == t2 { "same shape" } else { "different" });
  println(if t1 != t3 { "different shape" } else { "same" });
  println(if (1, "a") == (1, "a") { "tuples equal" } else { "tuples differ" });
  println(if "abc" == "ab" ++ "c" { "strings equal" } else { "strings differ" });
  let words = array_make(3, "");
  array_set(words, 0, "x");
  array_set(words, 2, "z");
  println(array_get(words, 0) ++ array_get(words, 1) ++ array_get(words, 2));
}
// expect: 5
// expect: lliuQ
// expect: pile
// expect: -1233
// expect: 285
// expect: same shape
// expect: different shape
// expect: tuples equal
// expect: strings equal
// expect: xz
