//! End-to-end tests: every program in `tests/programs` runs in the interpreter at each optimization level
//! (with the IR verified after every pass) and, when a C compiler is available, as native code at each
//! level. All runs must produce exactly the output written in the program's `// expect:` comments.

use std::path::{Path, PathBuf};

use quill::opt::OptLevel;

pub struct Expectation {
    pub stdout: String,
    pub error: Option<String>,
}

pub fn expectation(src: &str) -> Expectation {
    let mut stdout = String::new();
    let mut error = None;
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("// expect: ") {
            stdout.push_str(rest);
            stdout.push('\n');
        } else if line == "// expect:" {
            stdout.push('\n');
        } else if let Some(rest) = line.strip_prefix("// expect-error: ") {
            error = Some(rest.to_string());
        }
    }
    Expectation { stdout, error }
}

pub fn programs() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("programs");
    let mut files: Vec<PathBuf> =
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|e| e == "ql")).collect();
    files.sort();
    files
}

#[test]
fn interpreter_at_every_level() {
    let mut failures = Vec::new();
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let want = expectation(&src);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for level in [OptLevel::O0, OptLevel::O1, OptLevel::O2] {
            let (m, _) = match quill::compile_to_ir(&name, &src, level, true) {
                Ok(x) => x,
                Err(c) => {
                    failures.push(format!("{name}: does not compile: {:?}", c.diags.list.iter().map(|d| &d.message).collect::<Vec<_>>()));
                    break;
                }
            };
            let (out, trap) = quill::interp::run_program(&m);
            if out != want.stdout || trap.as_ref().map(|t| t.0.clone()) != want.error {
                failures.push(format!(
                    "{name} at {level:?}:\n--- expected\n{}{}\n--- got\n{}{}",
                    want.stdout,
                    want.error.as_deref().map(|e| format!("error: {e}")).unwrap_or_default(),
                    out,
                    trap.map(|t| format!("error: {}", t.0)).unwrap_or_default()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Runs a native executable; returns (stdout, the error message after "quill: " if it failed).
pub fn run_exe(exe: &std::path::Path, stress: bool) -> (String, Option<String>) {
    let mut cmd = std::process::Command::new(exe);
    if stress {
        cmd.env("QUILL_GC_STRESS", "1");
    }
    let out = cmd.output().expect("run the compiled program");
    let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let err = if out.status.success() {
        None
    } else {
        Some(
            stderr
                .lines()
                .find_map(|l| l.strip_prefix("quill: "))
                .map(str::to_string)
                .unwrap_or_else(|| format!("exit status {:?}: {stderr}", out.status.code())),
        )
    };
    (stdout, err)
}

pub fn build_native(name: &str, src: &str, level: OptLevel, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let (m, _) = quill::compile_to_ir(name, src, level, true).map_err(|_| "does not compile".to_string())?;
    let (asm, _) = quill::codegen::compile(&m, quill::codegen::x86::Abi::host(), level != OptLevel::O0);
    let exe = dir.join(format!("{}-{level:?}{}", name.trim_end_matches(".ql"), if cfg!(windows) { ".exe" } else { "" }));
    quill::driver::link(&asm, &exe)?;
    Ok(exe)
}

#[test]
fn native_code_at_every_level() {
    if quill::driver::find_cc().is_err() {
        eprintln!("skipping native tests: no C compiler");
        return;
    }
    let dir = std::env::temp_dir().join(format!("quill-tests-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut failures = Vec::new();
    for path in programs() {
        let src = std::fs::read_to_string(&path).unwrap();
        let want = expectation(&src);
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        for level in [OptLevel::O0, OptLevel::O1, OptLevel::O2] {
            let exe = match build_native(&name, &src, level, &dir) {
                Ok(e) => e,
                Err(e) => {
                    failures.push(format!("{name} at {level:?}: build failed: {e}"));
                    continue;
                }
            };
            let (out, err) = run_exe(&exe, false);
            if out != want.stdout || err != want.error {
                failures.push(format!(
                    "{name} at {level:?} (native):\n--- expected\n{}{}\n--- got\n{}{}",
                    want.stdout,
                    want.error.as_deref().map(|e| format!("error: {e}")).unwrap_or_default(),
                    out,
                    err.map(|t| format!("error: {t}")).unwrap_or_default()
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// With QUILL_GC_STRESS=1 the collector runs at every allocation, so any value the compiler keeps where
/// the collector cannot see it (a missed root) shows up as corrupted output.
#[test]
fn native_code_under_gc_stress() {
    if quill::driver::find_cc().is_err() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("quill-stress-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut failures = Vec::new();
    for name in ["gc_roots.ql", "closures.ql", "patterns.ql", "strings_arrays.ql", "user_panic.ql"] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("programs").join(name);
        let src = std::fs::read_to_string(&path).unwrap();
        let want = expectation(&src);
        for level in [OptLevel::O0, OptLevel::O2] {
            let exe = build_native(name, &src, level, &dir).unwrap();
            let (out, err) = run_exe(&exe, true);
            if out != want.stdout || err != want.error {
                failures.push(format!("{name} at {level:?} under GC stress:\n--- expected\n{}\n--- got\n{}{:?}", want.stdout, out, err));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
