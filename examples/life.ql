// Conway's Game of Life on a wrapping grid, stored in an array.

fn idx(w, h, x, y) { ((y + h) % h) * w + (x + w) % w }

fn neighbours(cells, w, h, x, y) {
  var n = 0;
  var dy = -1;
  while dy <= 1 {
    var dx = -1;
    while dx <= 1 {
      if (dx != 0 || dy != 0) && array_get(cells, idx(w, h, x + dx, y + dy)) { n = n + 1; }
      dx = dx + 1;
    }
    dy = dy + 1;
  }
  n
}

fn step(cells, w, h) {
  let next = array_make(w * h, false);
  var y = 0;
  while y < h {
    var x = 0;
    while x < w {
      let n = neighbours(cells, w, h, x, y);
      let alive = array_get(cells, idx(w, h, x, y));
      array_set(next, idx(w, h, x, y), n == 3 || (alive && n == 2));
      x = x + 1;
    }
    y = y + 1;
  }
  next
}

fn render(cells, w, h) {
  var y = 0;
  while y < h {
    var line = "";
    var x = 0;
    while x < w {
      line = line ++ (if array_get(cells, idx(w, h, x, y)) { "#" } else { "." });
      x = x + 1;
    }
    println(line);
    y = y + 1;
  }
}

fn population(cells) {
  var n = 0;
  var i = 0;
  while i < array_length(cells) {
    if array_get(cells, i) { n = n + 1; }
    i = i + 1;
  }
  n
}

fn main() {
  let w = 20;
  let h = 10;
  var cells = array_make(w * h, false);
  // A glider and a blinker.
  array_set(cells, idx(w, h, 1, 0), true);
  array_set(cells, idx(w, h, 2, 1), true);
  array_set(cells, idx(w, h, 0, 2), true);
  array_set(cells, idx(w, h, 1, 2), true);
  array_set(cells, idx(w, h, 2, 2), true);
  array_set(cells, idx(w, h, 12, 5), true);
  array_set(cells, idx(w, h, 13, 5), true);
  array_set(cells, idx(w, h, 14, 5), true);
  var gen = 0;
  while gen <= 8 {
    if gen % 4 == 0 {
      println("generation " ++ int_to_string(gen) ++ ", population " ++ int_to_string(population(cells)));
      render(cells, w, h);
    }
    cells = step(cells, w, h);
    gen = gen + 1;
  }
}
