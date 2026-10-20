//! Quill: a statically typed language with type inference, compiled to x86-64 machine code.

pub mod ast;
pub mod builtins;
pub mod codegen;
pub mod diag;
pub mod dom;
pub mod driver;
pub mod exhaust;
pub mod fuzz_cmd;
pub mod fuzzgen;
pub mod interp;
pub mod ir;
pub mod lexer;
pub mod lower;
pub mod lsp;
pub mod opt;
pub mod parser;
pub mod repl;
pub mod typeck;
pub mod types;

use diag::{Diagnostics, SourceFile};

/// The result of the front end: parsing and type checking.
pub struct Checked {
    pub file: SourceFile,
    pub typed: typeck::Typed,
    pub diags: Diagnostics,
}

/// Parses and type-checks a source file.
pub fn check_source(name: &str, text: &str) -> Checked {
    let file = SourceFile::new(name, text);
    let mut diags = Diagnostics::default();
    let program = parser::parse_program(text, &mut diags);
    let parse_failed = diags.has_errors();
    let typed = typeck::check(program, &mut diags);
    if parse_failed {
        // Keep only errors that are not knock-on effects of syntax errors.
        diags.list.retain(|d| d.is_error() || !d.message.starts_with("unused"));
    }
    Checked { file, typed, diags }
}

/// Parses, checks, lowers and optimizes. Returns the diagnostics as an error if checking failed.
#[allow(clippy::result_large_err)]
pub fn compile_to_ir(name: &str, text: &str, level: opt::OptLevel, verify: bool) -> Result<(ir::Module, Checked), Checked> {
    let checked = check_source(name, text);
    if checked.diags.has_errors() {
        return Err(checked);
    }
    let mut m = lower::lower(&checked.typed, &checked.file);
    if verify && let Err(e) = ir::verify(&m) {
        panic!("lowering produced invalid IR: {e}");
    }
    opt::optimize(&mut m, level, verify, None);
    Ok((m, checked))
}
