// The "binary trees" benchmark: allocates millions of short-lived tree nodes (a garbage collector test).
type Tree = Leaf | Node(Tree, Tree)

fn make(depth) { if depth == 0 { Node(Leaf, Leaf) } else { Node(make(depth - 1), make(depth - 1)) } }

fn check(t) { match t { Leaf => 0, Node(l, r) => 1 + check(l) + check(r) } }

fn pow2(n) { if n == 0 { 1 } else { 2 * pow2(n - 1) } }

fn main() {
  let max_depth = 18;
  let long_lived = make(max_depth);
  var depth = 4;
  while depth <= max_depth {
    let iterations = pow2(max_depth - depth + 4);
    var total = 0;
    var i = 0;
    while i < iterations {
      total = total + check(make(depth));
      i = i + 1;
    }
    println(int_to_string(iterations) ++ " trees of depth " ++ int_to_string(depth) ++ " check: " ++ int_to_string(total));
    depth = depth + 2;
  }
  println("long lived tree of depth " ++ int_to_string(max_depth) ++ " check: " ++ int_to_string(check(long_lived)));
}
