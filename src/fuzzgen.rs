//! A generator of random, well-typed, terminating Quill programs, for differential testing: the same
//! program must print the same thing in the interpreter and as native code at every optimization level.
//!
//! Programs use integer arithmetic (with overflow), comparisons, mutable variables, bounded loops,
//! bounded recursion, pattern matching, closures, tuples and arrays. Divisors are forced to be non-zero
//! and every loop and recursion has a fixed bound, so every program terminates without a run-time error.

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

struct Gen {
    rng: Rng,
    out: String,
    /// Integer variables in scope: (name, mutable). Loop counters are mutable but never assigned by
    /// generated code (that could make the loop infinite): they are listed as immutable in `ints` while in
    /// their loop, and in `counters`, so closures do not capture them.
    ints: Vec<(String, bool)>,
    counters: Vec<String>,
    next_name: u32,
    /// Functions defined so far (callable from later ones): name.
    fns: Vec<String>,
    /// The current function, if it may call itself (with a smaller first argument).
    recursive: Option<String>,
    in_closure: bool,
}

pub fn generate(seed: u64) -> String {
    let mut g = Gen {
        rng: Rng::new(seed),
        out: String::new(),
        ints: vec![],
        counters: vec![],
        next_name: 0,
        fns: vec![],
        recursive: None,
        in_closure: false,
    };
    g.program();
    g.out
}

impl Gen {
    fn name(&mut self, prefix: &str) -> String {
        self.next_name += 1;
        format!("{prefix}{}", self.next_name)
    }

    fn line(&mut self, indent: usize, s: &str) {
        for _ in 0..indent {
            self.out.push_str("  ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn program(&mut self) {
        self.line(0, "type Shape = Circle(Int) | Rect(Int, Int) | Dot");
        self.line(0, "fn area(s) { match s { Circle(r) => r * r * 3, Rect(w, h) => w * h, Dot => 1 } }");
        let nfns = 1 + self.rng.below(4);
        for _ in 0..nfns {
            self.function();
        }
        self.line(0, "fn main() {");
        self.ints.clear();
        self.recursive = None;
        self.line(1, "var checksum = 7;");
        self.ints.push(("checksum".into(), true));
        let n = 3 + self.rng.below(6);
        for _ in 0..n {
            self.stmt(1, 2);
        }
        self.line(1, "print_int(checksum);");
        self.line(0, "}");
    }

    fn function(&mut self) {
        let name = self.name("f");
        self.ints = vec![("n".into(), false), ("x".into(), false)];
        self.line(0, &format!("fn {name}(n: Int, x: Int) -> Int {{"));
        self.line(1, "if n <= 0 { x } else {");
        self.recursive = Some(name.clone());
        let k = 1 + self.rng.below(3);
        for _ in 0..k {
            self.stmt(2, 2);
        }
        let e = self.int(3);
        self.line(2, &e);
        self.line(1, "}");
        self.line(0, "}");
        self.recursive = None;
        self.fns.push(name);
    }

    fn mutable_var(&mut self) -> Option<String> {
        if self.in_closure {
            return None;
        }
        let vars: Vec<String> = self.ints.iter().filter(|(_, m)| *m).map(|(n, _)| n.clone()).collect();
        if vars.is_empty() { None } else { Some(vars[self.rng.below(vars.len() as u64) as usize].clone()) }
    }

    fn stmt(&mut self, indent: usize, depth: u32) {
        match self.rng.below(10) {
            0 | 1 => {
                let n = self.name("a");
                let e = self.int(3);
                self.line(indent, &format!("let {n} = {e};"));
                self.ints.push((n, false));
            }
            2 => {
                let n = self.name("v");
                let e = self.int(2);
                self.line(indent, &format!("var {n} = {e};"));
                self.ints.push((n, true));
            }
            3 | 4 => {
                if let Some(v) = self.mutable_var() {
                    let e = self.int(3);
                    self.line(indent, &format!("{v} = {e};"));
                } else {
                    let e = self.int(2);
                    self.line(indent, &format!("print_int({e});"));
                }
            }
            5 if depth > 0 => {
                // A bounded loop.
                let i = self.name("i");
                let bound = 1 + self.rng.below(6);
                self.line(indent, &format!("var {i} = 0;"));
                self.line(indent, &format!("while {i} < {bound} {{"));
                let mark = self.ints.len();
                self.ints.push((i.clone(), false));
                self.counters.push(i.clone());
                let k = 1 + self.rng.below(3);
                for _ in 0..k {
                    self.stmt(indent + 1, depth - 1);
                }
                self.ints.truncate(mark);
                self.line(indent + 1, &format!("{i} = {i} + 1;"));
                self.line(indent, "}");
                self.ints.push((i, true));
            }
            6 if depth > 0 => {
                let c = self.boolean(2);
                self.line(indent, &format!("if {c} {{"));
                let mark = self.ints.len();
                self.stmt(indent + 1, depth - 1);
                self.ints.truncate(mark);
                self.line(indent, "} else {");
                self.stmt(indent + 1, depth - 1);
                self.ints.truncate(mark);
                self.line(indent, "}");
            }
            7 => {
                let (a, b) = (self.name("t"), self.name("t"));
                let (x, y) = (self.int(2), self.int(2));
                self.line(indent, &format!("let ({a}, {b}) = ({x}, {y});"));
                self.ints.push((a, false));
                self.ints.push((b, false));
            }
            8 => {
                let arr = self.name("arr");
                let len = 1 + self.rng.below(5);
                let init = self.int(1);
                self.line(indent, &format!("let {arr} = array_make({len}, {init});"));
                let idx = self.rng.below(len);
                let v = self.int(2);
                self.line(indent, &format!("array_set({arr}, {idx}, {v});"));
                let n = self.name("a");
                let j = self.rng.below(len);
                self.line(indent, &format!("let {n} = array_get({arr}, {j}) + array_length({arr});"));
                self.ints.push((n, false));
            }
            _ => {
                if self.ints.iter().any(|(n, _)| n == "checksum") && !self.in_closure {
                    let e = self.int(3);
                    self.line(indent, &format!("checksum = checksum * 31 + {e};"));
                } else {
                    let e = self.int(2);
                    self.line(indent, &format!("print_int({e});"));
                }
            }
        }
    }

    fn literal(&mut self) -> String {
        match self.rng.below(10) {
            0 => format!("{}", 4_611_686_018_427_387_903i64 - self.rng.below(5) as i64),
            1 => format!("-{}", 4_611_686_018_427_387_904i64 - self.rng.below(5) as i64),
            2 => format!("{}", self.rng.below(1 << 40)),
            3 => format!("-{}", self.rng.below(1000)),
            _ => format!("{}", self.rng.below(20)),
        }
    }

    fn atom(&mut self) -> String {
        // Closures may only capture immutable variables.
        let usable: Vec<String> =
            self.ints.iter().filter(|(n, m)| !self.in_closure || (!*m && !self.counters.contains(n))).map(|(n, _)| n.clone()).collect();
        if !usable.is_empty() && self.rng.chance(65) {
            return usable[self.rng.below(usable.len() as u64) as usize].clone();
        }
        self.literal()
    }

    fn int(&mut self, depth: u32) -> String {
        if depth == 0 {
            return self.atom();
        }
        let d = depth - 1;
        match self.rng.below(16) {
            0 | 1 => self.atom(),
            2 => format!("({} + {})", self.int(d), self.int(d)),
            3 => format!("({} - {})", self.int(d), self.int(d)),
            4 => format!("({} * {})", self.int(d), self.int(d)),
            5 => {
                let divisor = self.divisor(d);
                format!("({} / {divisor})", self.int(d))
            }
            6 => {
                let divisor = self.divisor(d);
                format!("({} % {divisor})", self.int(d))
            }
            7 => format!("(-{})", self.int(d)),
            8 => format!("(if {} {{ {} }} else {{ {} }})", self.boolean(d), self.int(d), self.int(d)),
            9 if !self.fns.is_empty() => {
                let f = self.fns[self.rng.below(self.fns.len() as u64) as usize].clone();
                let n = self.rng.below(4);
                format!("{f}({n}, {})", self.int(d))
            }
            10 if self.recursive.is_some() => {
                let f = self.recursive.clone().unwrap();
                format!("{f}(n - 1, {})", self.int(d))
            }
            11 => {
                let s = match self.rng.below(3) {
                    0 => format!("Circle({})", self.int(d)),
                    1 => format!("Rect({}, {})", self.int(d), self.int(d)),
                    _ => "Dot".to_string(),
                };
                format!("area({s})")
            }
            12 => {
                // A closure capturing the variables in scope, called right away or through a let.
                let p = self.name("p");
                let saved = std::mem::replace(&mut self.in_closure, true);
                let mark = self.ints.len();
                self.ints.push((p.clone(), false));
                let body = self.int(d);
                self.ints.truncate(mark);
                self.in_closure = saved;
                let arg = self.int(d);
                format!("{{ let g = |{p}| {body}; g({arg}) }}")
            }
            13 => {
                let v = self.int(d);
                format!("(match {v} {{ 0 => 100, 1 => 200, k if k < 0 => k * 2, k => k + 1 }})")
            }
            14 => format!("{{ let (q, r) = ({}, {}); q - r }}", self.int(d), self.int(d)),
            _ => format!("({} + {})", self.atom(), self.literal()),
        }
    }

    /// An expression that is never zero: in [2, 10].
    fn divisor(&mut self, d: u32) -> String {
        if self.rng.chance(40) {
            let k = [2, 3, 4, 7, 8, 16, 1024][self.rng.below(7) as usize];
            return k.to_string();
        }
        format!("({} % 5 + 6)", self.int(d))
    }

    fn boolean(&mut self, depth: u32) -> String {
        let d = depth.saturating_sub(1);
        match self.rng.below(8) {
            0 => format!("{} < {}", self.int(d), self.int(d)),
            1 => format!("{} <= {}", self.int(d), self.int(d)),
            2 => format!("{} == {}", self.int(d), self.int(d)),
            3 => format!("{} != {}", self.int(d), self.int(d)),
            4 if depth > 0 => format!("({} && {})", self.boolean(d), self.boolean(d)),
            5 if depth > 0 => format!("({} || {})", self.boolean(d), self.boolean(d)),
            6 if depth > 0 => format!("!({})", self.boolean(d)),
            _ => {
                if self.rng.chance(50) {
                    "true".into()
                } else {
                    format!("{} > {}", self.int(d), self.int(d))
                }
            }
        }
    }
}
