// Many objects live at the same time across allocations: some stay in callee-saved registers, some are
// spilled to the stack, some only exist inside closures and arrays. Run with QUILL_GC_STRESS=1 (a
// collection at every allocation) this checks that the collector finds every root.
type Pair = Pair(Int, Int)
type List[a] = Nil | Cons(a, List[a])

fn mk(n) { Pair(n, n * n) }
fn snd(p) { match p { Pair(_, b) => b } }

fn many(n) {
  let a = mk(n);
  let b = mk(n + 1);
  let c = mk(n + 2);
  let d = mk(n + 3);
  let e = mk(n + 4);
  let f = mk(n + 5);
  let g = mk(n + 6);
  let h = mk(n + 7);
  let i = mk(n + 8);
  let j = mk(n + 9);
  let k = mk(n + 10);
  let l = mk(n + 11);
  // All of them are still needed after these allocations.
  let s1 = int_to_string(snd(a)) ++ "," ++ int_to_string(snd(b));
  let s2 = s1 ++ "," ++ int_to_string(snd(c) + snd(d) + snd(e) + snd(f));
  snd(a) + snd(b) + snd(c) + snd(d) + snd(e) + snd(f) + snd(g) + snd(h) + snd(i) + snd(j) + snd(k) + snd(l) + string_length(s2)
}

fn adders(n, acc) {
  if n == 0 { acc } else {
    let m = n * 10;
    adders(n - 1, Cons(|x| x + m, acc))
  }
}

fn apply_all(fs, x) { match fs { Nil => x, Cons(f, rest) => apply_all(rest, f(x)) } }

fn main() {
  var total = 0;
  var r = 0;
  while r < 20 {
    total = total + many(r);
    r = r + 1;
  }
  print_int(total);
  let fs = adders(30, Nil);
  let arr = array_make(5, Cons(mk(1), Nil));
  array_set(arr, 3, Cons(mk(7), Cons(mk(8), Nil)));
  gc_collect();
  print_int(apply_all(fs, 0));
  match array_get(arr, 3) {
    Cons(Pair(x, _), Cons(Pair(_, y), Nil)) => print_int(x + y),
    _ => println("lost"),
  }
}
// expect: 65039
// expect: 4650
// expect: 71
