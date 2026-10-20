use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use quill::diag::Renderer;
use quill::opt::OptLevel;

const USAGE: &str = "\
Quill: a statically typed language with type inference, compiled to x86-64 machine code.

Usage:
  quill run <file.ql> [-O0|-O1|-O2] [--interp] [--stats]   compile and run (default -O2)
  quill build <file.ql> [-o <exe>] [-O0|-O1|-O2] [--stats]  compile to an executable
  quill build <file.ql> --emit <what>                         print an intermediate form:
        tokens, ast, types, ir (after lowering), ir-opt (after optimization), asm
  quill build <file.ql> --passes                              print the IR after every optimization pass
  quill check <file.ql>                                       type-check and print every function's type
  quill repl                                                  interactive session
  quill lsp                                                   language server (used by the VS Code extension)
  quill fuzz [count] [-o dir]                                 differential testing on random programs
  quill version

Environment:
  QUILL_CC          C compiler used to assemble and link (default: gcc, clang or cc)
  QUILL_GC_STRESS   set to 1 when running a program: collect garbage at every allocation
  QUILL_GC_STATS    set to 1 when running a program: print collector statistics at exit
";

struct Args {
    cmd: String,
    file: Option<PathBuf>,
    out: Option<PathBuf>,
    level: OptLevel,
    emit: Option<String>,
    interp: bool,
    stats: bool,
    passes: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let cmd = it.next().unwrap_or_else(|| "help".into());
    let mut a = Args { cmd, file: None, out: None, level: OptLevel::O2, emit: None, interp: false, stats: false, passes: false };
    while let Some(x) = it.next() {
        match x.as_str() {
            "-O0" => a.level = OptLevel::O0,
            "-O1" => a.level = OptLevel::O1,
            "-O2" | "-O" => a.level = OptLevel::O2,
            "-o" => a.out = Some(it.next().ok_or("-o needs a file name")?.into()),
            "--emit" => a.emit = Some(it.next().ok_or("--emit needs one of: tokens, ast, types, ir, ir-opt, asm")?),
            "--interp" => a.interp = true,
            "--stats" => a.stats = true,
            "--passes" => a.passes = true,
            s if s.starts_with('-') => return Err(format!("unknown option `{s}`")),
            s => {
                if a.file.is_some() {
                    return Err(format!("unexpected argument `{s}`"));
                }
                a.file = Some(s.into());
            }
        }
    }
    Ok(a)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("quill: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match args.cmd.as_str() {
        "run" => cmd_run(&args),
        "build" => cmd_build(&args),
        "check" => cmd_check(&args),
        "repl" => quill::repl::run().map(|_| ExitCode::SUCCESS),
        "lsp" => quill::lsp::run().map(|_| ExitCode::SUCCESS),
        "fuzz" => cmd_fuzz(&args),
        "version" | "--version" | "-V" => {
            println!("quill {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        other => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("quill: {e}");
            ExitCode::FAILURE
        }
    }
}

fn read_source(args: &Args) -> Result<(String, String), String> {
    let file = args.file.as_ref().ok_or("no input file")?;
    let text = std::fs::read_to_string(file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    Ok((file.display().to_string(), text))
}

fn renderer() -> Renderer {
    Renderer { color: std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none() }
}

/// Parses and checks, printing diagnostics. None if there were errors.
fn front_end(name: &str, text: &str) -> Option<quill::Checked> {
    let checked = quill::check_source(name, text);
    let r = renderer();
    for d in &checked.diags.list {
        eprint!("{}", r.render(&checked.file, d));
    }
    if checked.diags.has_errors() {
        let n = checked.diags.list.iter().filter(|d| d.is_error()).count();
        eprintln!("{}", if n == 1 { "1 error".to_string() } else { format!("{n} errors") });
        None
    } else {
        Some(checked)
    }
}

fn cmd_check(args: &Args) -> Result<ExitCode, String> {
    let (name, text) = read_source(args)?;
    let Some(checked) = front_end(&name, &text) else { return Ok(ExitCode::FAILURE) };
    let mut adts: Vec<&quill::typeck::AdtInfo> = checked.typed.adts.iter().collect();
    adts.sort_by_key(|a| a.span);
    for a in adts {
        let params = if a.params.is_empty() { String::new() } else { format!("[{}]", a.params.join(", ")) };
        println!("type {}{params}", a.name);
    }
    for f in &checked.typed.fn_order {
        println!("fn {f} : {}", quill::types::show(&checked.typed.fns[f].scheme.ty));
    }
    Ok(ExitCode::SUCCESS)
}

fn default_output(file: &Path) -> PathBuf {
    let stem = file.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "a".into());
    let mut p = file.with_file_name(stem);
    if cfg!(windows) {
        p.set_extension("exe");
    }
    p
}

/// Compiles to IR (and optimizes), printing what `--emit` / `--passes` / `--stats` ask for.
/// Returns None when the program was only printed.
fn compile(args: &Args, name: &str, text: &str) -> Result<Option<(quill::ir::Module, quill::Checked)>, String> {
    match args.emit.as_deref() {
        Some("tokens") => {
            let mut d = quill::diag::Diagnostics::default();
            for t in quill::lexer::lex(text, &mut d) {
                println!("{:>5}..{:<5} {:?}", t.span.start, t.span.end, t.tok);
            }
            return Ok(None);
        }
        Some("ast") => {
            let mut d = quill::diag::Diagnostics::default();
            println!("{:#?}", quill::parser::parse_program(text, &mut d));
            return Ok(None);
        }
        _ => {}
    }
    let Some(checked) = front_end(name, text) else { return Err("compilation failed".into()) };
    if args.emit.as_deref() == Some("types") {
        for f in &checked.typed.fn_order {
            println!("fn {f} : {}", quill::types::show(&checked.typed.fns[f].scheme.ty));
        }
        return Ok(None);
    }
    let t0 = Instant::now();
    let mut m = quill::lower::lower(&checked.typed, &checked.file);
    if args.emit.as_deref() == Some("ir") {
        print!("{}", quill::ir::print_module(&m));
        return Ok(None);
    }
    let mut log = Vec::new();
    let stats = quill::opt::optimize(&mut m, args.level, cfg!(debug_assertions), if args.passes { Some(&mut log) } else { None });
    if args.passes {
        for p in log {
            println!(";;; after {}\n{}", p.name, p.ir);
        }
        return Ok(None);
    }
    if args.emit.as_deref() == Some("ir-opt") {
        print!("{}", quill::ir::print_module(&m));
        return Ok(None);
    }
    if args.stats {
        eprintln!(
            "optimizer ({:?}, {:.1} ms): {} folded, {} branches folded, {} CSE, {} dead removed, {} inlined, {} calls devirtualized, {} blocks merged, {} block parameters removed, {} tail calls",
            args.level,
            t0.elapsed().as_secs_f64() * 1000.0,
            stats.folded,
            stats.branches_folded,
            stats.cse,
            stats.dce,
            stats.inlined,
            stats.devirtualized,
            stats.blocks_merged,
            stats.params_removed,
            stats.tail_calls
        );
    }
    Ok(Some((m, checked)))
}

fn native(args: &Args, m: &quill::ir::Module, exe: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    let (asm, cstats) = quill::codegen::compile(m, quill::codegen::x86::Abi::host(), args.level != OptLevel::O0);
    if args.emit.as_deref() == Some("asm") {
        print!("{asm}");
        return Ok(());
    }
    let t1 = Instant::now();
    quill::driver::link(&asm, exe)?;
    if args.stats {
        eprintln!(
            "codegen: {} functions, {} machine instructions, {} spill slots ({:.1} ms); assemble + link {:.1} ms",
            cstats.functions,
            cstats.instructions,
            cstats.spilled,
            (t1 - t0).as_secs_f64() * 1000.0,
            t1.elapsed().as_secs_f64() * 1000.0
        );
    }
    Ok(())
}

fn cmd_build(args: &Args) -> Result<ExitCode, String> {
    let (name, text) = read_source(args)?;
    let Some((m, _)) = compile(args, &name, &text)? else { return Ok(ExitCode::SUCCESS) };
    let exe = args.out.clone().unwrap_or_else(|| default_output(args.file.as_ref().unwrap()));
    native(args, &m, &exe)?;
    if args.emit.is_none() {
        eprintln!("built {}", exe.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_run(args: &Args) -> Result<ExitCode, String> {
    let (name, text) = read_source(args)?;
    let Some((m, _)) = compile(args, &name, &text)? else { return Ok(ExitCode::SUCCESS) };
    if args.interp {
        let (out, trap) = quill::interp::run_program(&m);
        print!("{out}");
        if let Some(t) = trap {
            eprintln!("quill: {}", t.0);
            return Ok(ExitCode::FAILURE);
        }
        return Ok(ExitCode::SUCCESS);
    }
    let dir = std::env::temp_dir().join("quill-run");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stem = args.file.as_ref().unwrap().file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let exe = dir.join(format!("{stem}-{}{}", std::process::id(), if cfg!(windows) { ".exe" } else { "" }));
    native(args, &m, &exe)?;
    if args.emit.is_some() {
        return Ok(ExitCode::SUCCESS);
    }
    let status = std::process::Command::new(&exe).status().map_err(|e| format!("cannot run {}: {e}", exe.display()))?;
    let _ = std::fs::remove_file(&exe);
    let _ = std::fs::remove_file(exe.with_extension("s"));
    Ok(if status.success() { ExitCode::SUCCESS } else { ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8) })
}

fn cmd_fuzz(args: &Args) -> Result<ExitCode, String> {
    let count: u64 = match &args.file {
        Some(f) => f.display().to_string().parse().map_err(|_| "fuzz expects a number of programs".to_string())?,
        None => 200,
    };
    let seed: u64 = std::env::var("QUILL_FUZZ_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let out = args.out.clone().unwrap_or_else(|| PathBuf::from("fuzz-failures"));
    let failures = quill::fuzz_cmd::run(count, seed, &out)?;
    Ok(if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
