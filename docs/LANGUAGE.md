# The Quill language

Quill is a small, expression-oriented, statically typed language. Types are inferred (Hindley–Milner with
let-polymorphism), so annotations are optional everywhere. This page describes the whole language.

## Programs

A program is a list of type and function declarations, in any order. Execution starts at `fn main()`.

```quill
// Line comments, and /* block comments */.
fn main() {
  println("hello, world");
}
```

Functions can be mutually recursive and may be declared after they are used.

## Values and types

| Type          | Values                                            |
|---------------|---------------------------------------------------|
| `Int`         | 63-bit signed integers; arithmetic wraps around   |
| `Bool`        | `true`, `false`                                   |
| `String`      | immutable byte strings: `"tab\t, quote\", newline\n"` |
| `()`          | unit, the value of statements and of `println(...)` |
| `(A, B, ...)` | tuples: `(1, "one")`                              |
| `Array[a]`    | mutable, fixed-size arrays                        |
| `(A, B) -> C` | functions and closures                            |
| user types    | algebraic data types (below)                      |

String escapes: `\n`, `\t`, `\r`, `\0`, `\\`, `\"`.

## Functions and inference

```quill
fn add(a, b) { a + b }                    // inferred: (Int, Int) -> Int
fn twice(f, x) { f(f(x)) }                // inferred: ((a) -> a, a) -> a
fn first(p: (Int, String)) -> Int {       // annotations are optional, and checked
  match p { (n, _) => n }
}
```

A function body is a block, and the value of a block is its last expression. `return e` leaves the function
early.

Functions are polymorphic: `twice` works on any type `a`, and each use picks its own instantiation.
`let`-bound values are generalized too, when their definition is a syntactic value (the value restriction):
`let id = |x| x;` can be used at several types.

## Expressions

| Precedence (low → high) | Operators                        | Notes                           |
|-------------------------|----------------------------------|---------------------------------|
| 1                       | `\|\|`                           | short-circuit                   |
| 2                       | `&&`                             | short-circuit                   |
| 3                       | `==` `!=` `<` `<=` `>` `>=`      | not chainable: `a < b < c` is an error |
| 4                       | `++`                             | string concatenation, right-associative |
| 5                       | `+` `-`                          |                                 |
| 6                       | `*` `/` `%`                      | `/` and `%` truncate toward zero |
| prefix                  | `-` `!`                          |                                 |

`==` and `!=` compare structurally (strings by content, constructors field by field); arrays compare by
identity. Comparing functions is a type error. `<`, `<=`, `>`, `>=` need `Int`s.

`if` is an expression: `let m = if a > b { a } else { b };`. Without `else`, the branch must have type `()`.

Lambdas: `|x| x + 1`, `|a, b| a * b`, and `|| 42` with no parameters. They capture the variables they use.

## Variables

```quill
let x = 1;          // immutable
var i = 0;          // mutable
i = i + 1;
while i < 10 { i = i + 1; }
```

Assigning to a `let` is an error. Closures may not capture a `var` (copy it into a `let` first), which keeps
mutable state on the stack and makes closures safe to keep around.

## Algebraic data types and pattern matching

```quill
type Option[a] = None | Some(a)
type List[a] = Nil | Cons(a, List[a])
type Shape = Circle(Int) | Rect(Int, Int)

fn area(s) {
  match s {
    Circle(r) => 3 * r * r,
    Rect(w, h) if w == h => w * w,     // guards
    Rect(w, h) => w * h,
  }
}
```

Patterns: `_`, variables, integer, string and boolean literals, tuples and constructors, nested to any depth.
Arms are tried in order. The compiler checks every `match`:

- a `match` that does not cover every case is an **error**, and the message names a missing case, such as
  ``Cons(_, Nil)` is not handled``;
- an arm that can never be reached is reported as a warning.

## Built-in functions

| Function | Type | |
|---|---|---|
| `print(s)`, `println(s)` | `(String) -> ()` | write to standard output |
| `print_int(n)` | `(Int) -> ()` | an integer and a newline |
| `int_to_string(n)` | `(Int) -> String` | |
| `string_to_int(s)` | `(String) -> Int` | fails on malformed input |
| `string_length(s)` | `(String) -> Int` | in bytes |
| `string_get(s, i)` | `(String, Int) -> Int` | the byte at `i`; bounds-checked |
| `substring(s, start, len)` | `(String, Int, Int) -> String` | bounds-checked |
| `char_to_string(c)` | `(Int) -> String` | a one-byte string |
| `array_make(n, x)` | `(Int, a) -> Array[a]` | n copies of x |
| `array_get(a, i)`, `array_set(a, i, x)` | | bounds-checked |
| `array_length(a)` | `(Array[a]) -> Int` | |
| `panic(msg)` | `(String) -> a` | stops the program |
| `assert(cond)` | `(Bool) -> ()` | |
| `clock_ms()` | `() -> Int` | a monotonic clock |
| `gc_collect()`, `gc_count()` | | run the collector; count collections |

## Run-time errors

Division by zero, out-of-range indexes, failed assertions, `panic`, a `match` that falls through (which cannot
happen in a checked program) and stack overflow stop the program with a message on standard error, such as
`quill: array index 3 out of bounds (length 3)`, and exit status 1. Native code and the interpreter report
the same messages.
