# Quill

[![CI](https://github.com/YUStulinu/Quill/actions/workflows/ci.yml/badge.svg)](https://github.com/YUStulinu/Quill/actions/workflows/ci.yml)

A compiler for **Quill**, a small statically typed functional language. Types are inferred, and programs
compile to **native x86-64 code** with no LLVM: the compiler has its own SSA optimizer, register allocator
and assembly emitter, and links against a small C runtime with a mark-and-sweep garbage collector. It also
comes with an interpreter, a REPL, a language server and a VS Code extension.

```quill
type List[a] = Nil | Cons(a, List[a])

fn map(xs, f) {
  match xs {
    Nil => Nil,
    Cons(x, rest) => Cons(f(x), map(rest, f)),
  }
}

fn sum(xs, acc) {
  match xs { Nil => acc, Cons(x, rest) => sum(rest, acc + x) }
}

fn main() {
  let xs = map(Cons(1, Cons(2, Cons(3, Nil))), |x| x * x);
  print_int(sum(xs, 0));   // 14
}
```

```
$ quill check demo.ql
type List[a]
fn map : (List[a], (a) -> b) -> List[b]
fn sum : (List[Int], Int) -> Int
fn main : () -> ()
$ quill run demo.ql
14
```

The language is described in [docs/LANGUAGE.md](docs/LANGUAGE.md).

## Features

- **Language:** algebraic data types, pattern matching with guards, closures, tuples, arrays, strings,
  mutable local variables and `while` loops. Hindley–Milner type inference with let-polymorphism, so
  type annotations are never required.
- **Diagnostics:** errors are shown rustc-style, with the source line, a caret and notes. Examples:
  - type mismatches, explained in terms of the code;
  - "did you mean" suggestions for misspelled names;
  - exhaustiveness checking that names the missing case, such as ``Cons(_, Nil)` is not handled``;
  - warnings for unreachable match arms and unused variables.
- **Compiler pipeline:** typed AST, then SSA IR with block parameters, then an optimizer, then machine IR,
  then linear-scan register allocation, then GNU assembly. The compiler targets both the Windows x64 and
  the System V calling conventions.
- **Optimizer:**
  - constant propagation (SCCP), instruction combining and global value numbering;
  - dead code elimination and CFG simplification;
  - inlining, closure devirtualization and tail-call elimination;
  - division by constants through multiplication by a magic number, and inline bounds-checked array and
    string accesses.
- **Runtime:** a precise heap with conservative stack scanning, a mark-and-sweep collector with bitmap
  allocation and size classes, and run-time errors with clear messages (bounds, division by zero, stack
  overflow).
- **Tooling:**
  - `quill repl`;
  - `quill lsp`, a language server with errors as you type, types on hover, inlay hints for inferred types,
    go to definition, completion and document symbols;
  - a VS Code extension;
  - `quill fuzz`, a differential fuzzer that generates random well-typed programs and checks that the
    interpreter and native code agree at every optimization level.

## Getting started

Requirements:

- Rust (stable, 2024 edition).
- A C compiler to assemble and link: GCC or Clang. On Windows, use MinGW-w64, for example MSYS2's
  `mingw-w64-ucrt-x86_64-gcc`.

The compiler looks for `gcc`, `clang` or `cc` on the PATH. To use a specific one, set `QUILL_CC`.

```bash
cargo build --release
cargo install --path .           # puts `quill` on the PATH
quill run examples/lists.ql
```

Commands:

```
quill run <file.ql> [-O0|-O1|-O2] [--interp] [--stats]   compile and run (default -O2)
quill build <file.ql> [-o <exe>] [-O0|-O1|-O2]            compile to an executable
quill build <file.ql> --emit tokens|ast|types|ir|ir-opt|asm
quill build <file.ql> --passes                            the IR after every optimization pass
quill check <file.ql>                                     type-check and print every function's type
quill repl                                                interactive session (:type, :ir, :asm, :defs, :load, :time)
quill lsp                                                 language server
quill fuzz [count] [-o dir]                               differential testing on random programs
```

`--interp` runs the reference interpreter instead of native code. `--stats` prints what the optimizer did
and how long each stage took.

## VS Code

The extension in [editors/vscode](editors/vscode) provides the following:

- syntax highlighting;
- errors and warnings as you type;
- the inferred type of anything under the mouse, and inferred types shown inline;
- go to definition, completion and an outline of the file;
- commands **Quill: Run File** (the ▶ button in the editor title), **Show Optimized IR** and **Show Assembly**.

To install it:

```bash
cd editors/vscode && npm install && npm run package
code --install-extension quill-lang.vsix
```

The extension uses `quill` from the PATH. When `target/release/quill` exists in the open folder, it uses that
instead. You can also set an explicit path with the `quill.path` setting.

## How it works

| Stage | Source | |
|---|---|---|
| Lexer, parser | [lexer.rs](src/lexer.rs), [parser.rs](src/parser.rs) | Recursive descent with Pratt parsing for expressions; recovers from errors to report several per file |
| Type checker | [typeck.rs](src/typeck.rs), [types.rs](src/types.rs) | Algorithm J with levels for fast generalization; functions are checked one strongly connected component of the call graph at a time, so mutually recursive functions are inferred together and stay polymorphic for later uses |
| Exhaustiveness | [exhaust.rs](src/exhaust.rs) | Maranget's usefulness algorithm, which also produces the missing pattern |
| Lowering | [lower.rs](src/lower.rs) | Typed AST to SSA IR; mutable variables become SSA values (Braun et al.), and closures are converted to environment records |
| Optimizer | [opt.rs](src/opt.rs) | See the next section; every pass is followed by an IR verifier in tests |
| Instruction selection | [codegen/isel.rs](src/codegen/isel.rs) | Tagged integers: an Int n is stored as 2n+1, so the GC can tell integers from pointers |
| Register allocation | [codegen/regalloc.rs](src/codegen/regalloc.rs) | Linear scan on liveness intervals; values live across calls go to callee-saved registers |
| Emission | [codegen/emit.rs](src/codegen/emit.rs) | Frames, calling conventions (Win64 and System V), parallel moves for block arguments |
| Runtime | [runtime/quill_rt.c](runtime/quill_rt.c) | Allocator, garbage collector, strings, arrays, errors; compiled once and cached |
| Interpreter | [interp.rs](src/interp.rs) | Runs the IR directly; the reference that native code is tested against |

### Optimizations

`-O0` translates the IR without changes, and is the baseline the tests compare against. `-O1` runs one round
of the passes, and `-O2` runs three rounds plus inlining and GVN.

| Pass | Effect |
|---|---|
| simplify-cfg | merges straight-line blocks, threads jumps, removes unused or constant block parameters |
| sccp | sparse conditional constant propagation: folds constants through branches and block parameters |
| instcombine | algebraic identities; turns a call through a known closure into a direct call |
| gvn | removes repeated computations, scoped by the dominator tree |
| dce | removes unused pure instructions and block parameters |
| inline | inlines small non-recursive functions |
| tail-calls | turns self-recursive calls in tail position into loops, so tail-recursive functions run in constant stack space |
| codegen | division by constants through multiplication by a magic number (no `idiv`); inline bounds-checked `array_get`, `array_set`, `string_get` and the length functions |

## Testing

`cargo test --release` runs these tests:

- **Unit tests:**
  - parser precedence and error recovery;
  - the magic-number division, checked against `idiv` on thousands of dividends and divisors.
- **Programs** ([tests/programs](tests/programs)): each file states its expected output or error. Every
  program runs in the interpreter at each optimization level, as native code at each level, and once more
  with `QUILL_GC_STRESS=1`, which collects at every allocation to expose missing GC roots.
- **Diagnostics** ([tests/errors](tests/errors)): each file lists the errors it must produce.
- **Language server:** an end-to-end JSON-RPC session that checks diagnostics, hover, definition, completion
  and inlay hints.
- **Fuzzing:** random programs from [fuzzgen.rs](src/fuzzgen.rs) with loops, closures, data types, matches,
  arrays and strings. Each must give the same output in the interpreter (`-O0` and `-O2`) and as native code
  at `-O0`, `-O1` and `-O2`. `quill fuzz 1000` runs the same check at a larger scale and saves any failing
  program.

CI runs everything on Windows and Linux, together with 300 fuzzed programs.

## Benchmarks

`python bench/run.py` builds each program in [bench](bench) at `-O0` and `-O2` and runs it. It also runs the
same algorithm in Python and, for two of the programs, in C. It checks that every version prints the same
result. The table shows the best of 5 runs, in wall-clock seconds, on an Intel Core i7-10870H under
Windows 11:

| benchmark | Quill -O0 | Quill -O2 | Python 3 | C (gcc -O2) |
|---|---:|---:|---:|---:|
| fib (recursive calls) | 0.040 | 0.038 | 0.37 | 0.025 |
| loops (integer arithmetic, `%` by constants) | 0.468 | 0.208 | 12.78 | 0.101 |
| sieve (array writes) | 0.465 | 0.388 | 3.72 | – |
| binary_trees (allocation and GC) | 1.981 | 2.059 | 18.34 | – |
| closures (higher-order functions) | 0.566 | 0.442 | 1.35 | – |
| strings (concatenation, `string_get`) | 0.322 | 0.312 | 1.13 | – |

Optimized Quill is 3 to 60 times faster than Python, and within about 2x of C on the two programs that
have a C version.

The optimizer gains the most where the program does arithmetic in loops or calls closures. The other programs
spend their time elsewhere:

- `binary_trees` spends it in the allocator and the collector;
- `sieve` is bound by memory, on a 16 MB array.

`QUILL_GC_STATS=1` shows the collector's share.

## License

MIT
