//! `quill fuzz`: differential testing on random programs. Every program must print the same in the
//! interpreter at -O0 (the reference), in the interpreter at -O2, and as native code at every level.
//! Failing programs are saved, with the reason in a comment at the top.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::opt::OptLevel;

pub fn run(count: u64, start_seed: u64, out_dir: &Path) -> Result<u64, String> {
    let work = std::env::temp_dir().join(format!("quill-fuzz-{}", std::process::id()));
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let mut failures = 0;
    let t0 = Instant::now();
    for seed in start_seed..start_seed + count {
        let src = crate::fuzzgen::generate(seed);
        if let Some(why) = check_one(seed, &src, &work)? {
            failures += 1;
            save_failure(out_dir, seed, &src, &why);
        }
        let done = seed - start_seed + 1;
        if done.is_multiple_of(25) {
            eprintln!("{done} programs, {failures} failures ({:.0} s)", t0.elapsed().as_secs_f64());
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    eprintln!("{count} programs checked, {failures} failures");
    Ok(failures)
}

/// Checks one program. Returns why it failed, if it did.
pub fn check_one(seed: u64, src: &str, work: &Path) -> Result<Option<String>, String> {
    let name = format!("fuzz-{seed}.ql");
    let reference = match crate::compile_to_ir(&name, src, OptLevel::O0, true) {
        Ok((m, _)) => crate::interp::run_program(&m),
        Err(c) => {
            let msgs: Vec<String> = c.diags.list.iter().filter(|d| d.is_error()).map(|d| d.message.clone()).collect();
            return Ok(Some(format!("does not compile: {msgs:?}")));
        }
    };
    let mut problems = Vec::new();
    if let Some(t) = &reference.1 {
        problems.push(format!("the reference run failed: {}", t.0));
    }
    for level in [OptLevel::O0, OptLevel::O1, OptLevel::O2] {
        let Ok((m, _)) = crate::compile_to_ir(&name, src, level, true) else { continue };
        if level == OptLevel::O2 {
            let r = crate::interp::run_program(&m);
            if r.0 != reference.0 {
                problems.push(format!("the interpreter at O2 differs from O0:\n--- O0\n{}--- O2\n{}", reference.0, r.0));
            }
        }
        if crate::driver::find_cc().is_err() {
            continue;
        }
        let (asm, _) = crate::codegen::compile(&m, crate::codegen::x86::Abi::host(), level != crate::opt::OptLevel::O0);
        let exe: PathBuf = work.join(format!("fuzz-{seed}-{level:?}{}", if cfg!(windows) { ".exe" } else { "" }));
        if let Err(e) = crate::driver::link(&asm, &exe) {
            problems.push(format!("native {level:?} does not link: {e}"));
            continue;
        }
        let out = std::process::Command::new(&exe).output().map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&out.stdout).replace("\r\n", "\n");
        if stdout != reference.0 || !out.status.success() {
            problems.push(format!(
                "native {level:?} differs:\n--- interpreter\n{}--- native\n{}{}",
                reference.0,
                stdout,
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::remove_file(exe.with_extension("s"));
    }
    Ok(if problems.is_empty() { None } else { Some(problems.join("\n")) })
}

fn save_failure(dir: &Path, seed: u64, src: &str, why: &str) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(format!("seed-{seed}.ql"));
    let commented: String = why.lines().map(|l| format!("// {l}\n")).collect();
    let _ = std::fs::write(&path, format!("{commented}{src}"));
    eprintln!("seed {seed}: FAILED, saved to {}", path.display());
}
