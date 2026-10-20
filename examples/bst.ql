// A polymorphic binary search tree, ordered by a comparison function passed in as a closure.

type Tree[a] = Leaf | Node(Tree[a], a, Tree[a])
type List[a] = Nil | Cons(a, List[a])

// cmp(a, b) < 0 when a comes before b
fn insert(t, x, cmp) {
  match t {
    Leaf => Node(Leaf, x, Leaf),
    Node(l, v, r) => {
      let c = cmp(x, v);
      if c < 0 { Node(insert(l, x, cmp), v, r) }
      else if c > 0 { Node(l, v, insert(r, x, cmp)) }
      else { t }
    }
  }
}

fn contains(t, x, cmp) {
  match t {
    Leaf => false,
    Node(l, v, r) => {
      let c = cmp(x, v);
      if c == 0 { true } else if c < 0 { contains(l, x, cmp) } else { contains(r, x, cmp) }
    }
  }
}

fn to_list(t, acc) {
  match t {
    Leaf => acc,
    Node(l, v, r) => to_list(l, Cons(v, to_list(r, acc))),
  }
}

fn size(t) { match t { Leaf => 0, Node(l, _, r) => size(l) + 1 + size(r) } }
fn height(t) { match t { Leaf => 0, Node(l, _, r) => 1 + max(height(l), height(r)) } }
fn max(a, b) { if a > b { a } else { b } }

fn from_list(xs, cmp) {
  var t = Leaf;
  var rest = xs;
  var go = true;
  while go {
    match rest {
      Nil => { go = false; }
      Cons(x, more) => { t = insert(t, x, cmp); rest = more; }
    }
  }
  t
}

fn join(xs, show) {
  match xs {
    Nil => "",
    Cons(x, Nil) => show(x),
    Cons(x, rest) => show(x) ++ " " ++ join(rest, show),
  }
}

// A pseudo-random sequence (a linear congruential generator).
fn randoms(n, seed) {
  if n == 0 { Nil } else {
    let next = (seed * 1103515245 + 12345) % 2147483648;
    Cons(next % 1000, randoms(n - 1, next))
  }
}

fn main() {
  let by_value = |a, b| a - b;
  let numbers = from_list(randoms(2000, 42), by_value);
  println("distinct numbers: " ++ int_to_string(size(numbers)) ++ ", height " ++ int_to_string(height(numbers)));
  println("contains 500? " ++ (if contains(numbers, 500, by_value) { "yes" } else { "no" }));

  // The same tree code, for strings ordered by length.
  let by_length = |a, b| string_length(a) - string_length(b);
  let words = from_list(Cons("compiler", Cons("ssa", Cons("garbage", Cons("inference", Cons("x86", Cons("lexer", Nil)))))), by_length);
  println("words by length: " ++ join(to_list(words, Nil), |w| w));

  let small = from_list(Cons(5, Cons(3, Cons(8, Cons(1, Cons(4, Nil))))), by_value);
  println("in order: " ++ join(to_list(small, Nil), int_to_string));
}
