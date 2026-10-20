// The N-queens puzzle: how many ways can N queens share an N x N board without attacking each other?
// Backtracking over columns, with the placed queens' rows kept in an array.

fn safe(rows, col, row) {
  var c = 0;
  var ok = true;
  while c < col && ok {
    let r = array_get(rows, c);
    if r == row || r - row == col - c || row - r == col - c { ok = false; }
    c = c + 1;
  }
  ok
}

fn solve(rows, col, n) {
  if col == n {
    1
  } else {
    var count = 0;
    var row = 0;
    while row < n {
      if safe(rows, col, row) {
        array_set(rows, col, row);
        count = count + solve(rows, col + 1, n);
      }
      row = row + 1;
    }
    count
  }
}

fn main() {
  var n = 4;
  while n <= 10 {
    let rows = array_make(n, 0);
    println(int_to_string(n) ++ " queens: " ++ int_to_string(solve(rows, 0, n)) ++ " solutions");
    n = n + 1;
  }
}
