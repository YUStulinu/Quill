//! The interactive REPL.
//!
//! Definitions (`fn`, `type`) accumulate in the session. Every other input is a statement or expression:
//! it is appended to the session's statements, and the whole sequence is compiled into one function and
//! run by the interpreter, so `let` bindings and mutations (`array_set`) carry over from one input to the
//! next. The output of earlier inputs is not shown again (it is the same every time, since programs are
//! deterministic apart from `clock_ms`).

use std::io::Write;
use std::time::Instant;

use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::diag::{Renderer, SourceFile};
use crate::interp::{Interp, show_value};
use crate::opt::OptLevel;
use crate::types::{Ty, show};

const HELP: &str = "\
Enter definitions (fn, type), statements (let x = ...;) or expressions (1 + 2).
Commands:
  :type <expr>   show the type of an expression without running it
  :ir <expr>     show the optimized IR of an expression
  :asm <expr>    show the x86-64 assembly of an expression
  :defs          list the session's definitions and bindings
  :load <file>   load the definitions of a file (everything but its main)
  :time          toggle timing of every evaluation
  :reset         forget everything
  :quit          leave (or Ctrl+D)";

#[derive(Default)]
struct Session {
    defs: Vec<String>,
    stmts: Vec<String>,
    /// Output produced by replaying `stmts` (hidden on the next run).
    prefix_out: usize,
    timing: bool,
}

fn paint(code: &str, s: &str) -> String {
    format!("\x1b[{code}m{s}\x1b[0m")
}

impl Session {
    fn program(&self, body: &str) -> String {
        let mut src = String::new();
        for d in &self.defs {
            src.push_str(d);
            src.push('\n');
        }
        let has_main = self.defs.iter().any(|d| d.trim_start().starts_with("fn main("));
        src.push_str("fn __repl() {\n");
        for s in &self.stmts {
            src.push_str(s);
            src.push('\n');
        }
        src.push_str(body);
        src.push_str("\n}\n");
        if !has_main {
            src.push_str("fn main() { }\n");
        }
        src
    }

    fn report(&self, src: &str, diags: &crate::diag::Diagnostics) -> bool {
        let file = SourceFile::new("<repl>", src);
        let r = Renderer { color: true };
        let mut errors = false;
        for d in diags.list.iter().filter(|d| d.is_error()) {
            errors = true;
            eprint!("{}", r.render(&file, d));
        }
        errors
    }
}

/// Is the input unfinished (an open brace, bracket or parenthesis, or a string)?
fn incomplete(s: &str) -> bool {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escape = false;
    for c in s.chars() {
        if in_str {
            match (escape, c) {
                (true, _) => escape = false,
                (false, '\\') => escape = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth > 0 || in_str
}

/// Runs the REPL on a thread with a large stack: the interpreter is recursive, like Quill programs.
pub fn run() -> Result<(), String> {
    std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(run_inner)
        .map_err(|e| e.to_string())?
        .join()
        .map_err(|_| "the REPL crashed".to_string())?
}

fn run_inner() -> Result<(), String> {
    println!("Quill {} REPL. Type :help for help, :quit to leave.", env!("CARGO_PKG_VERSION"));
    let mut rl = DefaultEditor::new().map_err(|e| e.to_string())?;
    let history = std::env::temp_dir().join("quill-repl-history.txt");
    let _ = rl.load_history(&history);
    let mut s = Session::default();
    loop {
        let mut input = match rl.readline("quill> ") {
            Ok(l) => l,
            Err(ReadlineError::Interrupted) => continue,
            Err(ReadlineError::Eof) => break,
            Err(e) => return Err(e.to_string()),
        };
        while incomplete(&input) {
            match rl.readline("  ...> ") {
                Ok(l) => {
                    input.push('\n');
                    input.push_str(&l);
                }
                Err(_) => break,
            }
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            continue;
        }
        let _ = rl.add_history_entry(trimmed);
        if !command(&mut s, trimmed) {
            break;
        }
        let _ = std::io::stdout().flush();
    }
    let _ = rl.save_history(&history);
    Ok(())
}

/// Handles one input. Returns false to quit.
fn command(s: &mut Session, input: &str) -> bool {
    if let Some(rest) = input.strip_prefix(':') {
        let (cmd, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let arg = arg.trim();
        match cmd {
            "q" | "quit" | "exit" => return false,
            "h" | "help" => println!("{HELP}"),
            "reset" => {
                *s = Session { timing: s.timing, ..Session::default() };
                println!("session cleared");
            }
            "time" => {
                s.timing = !s.timing;
                println!("timing {}", if s.timing { "on" } else { "off" });
            }
            "defs" => {
                for d in &s.defs {
                    println!("{d}");
                }
                for st in &s.stmts {
                    println!("{st}");
                }
            }
            "type" | "t" => show_type(s, arg),
            "ir" => show_code(s, arg, false),
            "asm" => show_code(s, arg, true),
            "load" => match std::fs::read_to_string(arg) {
                Ok(text) => load(s, &text),
                Err(e) => eprintln!("cannot read {arg}: {e}"),
            },
            other => eprintln!("unknown command :{other} (try :help)"),
        }
        return true;
    }
    let first = input.split_whitespace().next().unwrap_or("");
    if first == "fn" || first == "type" {
        define(s, input);
    } else {
        evaluate(s, input);
    }
    true
}

fn define(s: &mut Session, input: &str) {
    let mut candidate = s.defs.clone();
    candidate.push(input.to_string());
    let trial = Session { defs: candidate, stmts: s.stmts.clone(), ..Session::default() };
    let src = trial.program("()");
    let checked = crate::check_source("<repl>", &src);
    if trial.report(&src, &checked.diags) {
        return;
    }
    // Show what was defined.
    let mut diags = crate::diag::Diagnostics::default();
    let prog = crate::parser::parse_program(input, &mut diags);
    for item in prog.items {
        match item {
            crate::ast::Item::Fn(f) => {
                if let Some(info) = checked.typed.fns.get(&f.name.name) {
                    println!("{} : {}", paint("1", &f.name.name), paint("36", &show(&info.scheme.ty)));
                }
            }
            crate::ast::Item::Type(t) => println!("type {}", paint("1", &t.name.name)),
        }
    }
    s.defs.push(input.to_string());
}

fn load(s: &mut Session, text: &str) {
    let mut diags = crate::diag::Diagnostics::default();
    let prog = crate::parser::parse_program(text, &mut diags);
    if diags.has_errors() {
        eprintln!("the file has syntax errors; run `quill check` on it");
        return;
    }
    // Re-slice the source by item spans, skipping main.
    let mut n = 0;
    for item in &prog.items {
        let span = match item {
            crate::ast::Item::Fn(f) if f.name.name == "main" => continue,
            crate::ast::Item::Fn(f) => f.span,
            crate::ast::Item::Type(t) => t.span,
        };
        s.defs.push(text[span.start as usize..span.end as usize].to_string());
        n += 1;
    }
    println!("loaded {n} definitions");
}

/// The statement form of an input (with a `;` so it can be followed by more), and the expression whose
/// value to show, if any.
fn split_input(input: &str) -> (String, String) {
    let t = input.trim();
    let first = t.split_whitespace().next().unwrap_or("");
    if first == "let" || first == "var" {
        // Show the bound variable when it is a simple name.
        let name = t.split_whitespace().nth(1).unwrap_or("").trim_end_matches(':').to_string();
        let stmt = if t.ends_with(';') { t.to_string() } else { format!("{t};") };
        let shown =
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.is_empty() && name != "_" { name } else { "()".into() };
        return (stmt, shown);
    }
    if t.ends_with(';') || first == "while" {
        return (t.to_string(), "()".into());
    }
    (format!("let __it = {{ {t} }};"), "__it".into())
}

fn evaluate(s: &mut Session, input: &str) {
    let (stmt, shown) = split_input(input);
    let body = format!("{stmt}\n{shown}");
    let src = s.program(&body);
    let (m, checked) = match crate::compile_to_ir("<repl>", &src, OptLevel::O1, false) {
        Ok(x) => x,
        Err(c) => {
            s.report(&src, &c.diags);
            return;
        }
    };
    let Some(fid) = m.funcs.iter().position(|f| f.name == "__repl") else { return };
    let ret_ty = match &checked.typed.fns["__repl"].scheme.ty {
        Ty::Fn(_, r) => (**r).clone(),
        _ => Ty::unit(),
    };
    let started = Instant::now();
    let mut out: Vec<u8> = Vec::new();
    let result = {
        let mut it = Interp::new(&m, &mut out);
        it.call(fid as u32, vec![])
    };
    let new_out = &out[s.prefix_out.min(out.len())..];
    let _ = std::io::stdout().write_all(new_out);
    if !new_out.is_empty() && !new_out.ends_with(b"\n") {
        println!();
    }
    match result {
        Ok(v) => {
            if !(ret_ty.is_unit() && (shown == "__it" || shown == "()")) {
                let value = show_value(&v, &ret_ty, &checked.typed);
                let label = if shown == "__it" || shown == "()" { String::new() } else { format!("{} ", paint("1", &shown)) };
                println!("{label}{} {} {}", paint("2", ":"), paint("36", &show(&ret_ty)), paint("2", &format!("= {value}")));
            }
            s.stmts.push(stmt);
            s.prefix_out = out.len();
            if s.timing {
                println!("{}", paint("2", &format!("({:.2} ms)", started.elapsed().as_secs_f64() * 1000.0)));
            }
        }
        Err(t) => eprintln!("{} {}", paint("1;31", "error:"), t.0),
    }
}

fn show_type(s: &Session, expr: &str) {
    let src = s.program(&format!("let __it = {{ {expr} }};\n__it"));
    let checked = crate::check_source("<repl>", &src);
    if s.report(&src, &checked.diags) {
        return;
    }
    if let Some(Ty::Fn(_, r)) = checked.typed.fns.get("__repl").map(|f| f.scheme.ty.clone()) {
        println!("{}", paint("36", &show(&r)));
    }
}

fn show_code(s: &Session, expr: &str, asm: bool) {
    // Compile the expression as a function of its own, without the session's statements.
    let mut src = String::new();
    for d in &s.defs {
        src.push_str(d);
        src.push('\n');
    }
    src.push_str(&format!("fn __expr() {{ {expr} }}\n"));
    if !s.defs.iter().any(|d| d.trim_start().starts_with("fn main(")) {
        src.push_str("fn main() { }\n");
    }
    let (m, _) = match crate::compile_to_ir("<repl>", &src, OptLevel::O2, false) {
        Ok(x) => x,
        Err(c) => {
            s.report(&src, &c.diags);
            return;
        }
    };
    let Some(fid) = m.funcs.iter().position(|f| f.name == "__expr") else { return };
    if asm {
        let (text, _) = crate::codegen::compile(&m, crate::codegen::x86::Abi::host(), true);
        let sym = crate::codegen::Symbols::new(&m).func(fid as u32);
        let mut on = false;
        for line in text.lines() {
            if line.starts_with(&format!("{sym}:")) {
                on = true;
            } else if on && line.starts_with("    .globl") {
                break;
            }
            if on {
                println!("{line}");
            }
        }
    } else {
        print!("{}", crate::ir::print_function(&m, &m.funcs[fid]));
    }
}
