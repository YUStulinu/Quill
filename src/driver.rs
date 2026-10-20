//! Turning assembly into an executable: finds a C toolchain (only its assembler and linker are needed,
//! plus a C compiler for the run-time library, which is built once and cached).

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const RUNTIME_SOURCE: &str = include_str!("../runtime/quill_rt.c");

/// Finds a C compiler that can build the run-time library: `$QUILL_CC` if set, otherwise the first that
/// works of `gcc` on the PATH, MSYS2's GCC (on Windows), then `clang` and `cc` on the PATH.
///
/// "Works" means it compiled the run-time library, not just that it starts: on Windows, a `clang` from an
/// LLVM install often has no C headers or libraries. The library is cached, so the check is free after
/// the first time, and compilers that failed are remembered too.
pub fn find_cc() -> Result<PathBuf, String> {
    if let Ok(cc) = std::env::var("QUILL_CC")
        && !cc.is_empty()
    {
        return Ok(PathBuf::from(cc));
    }
    let mut msg = String::from(
        "no working C compiler found. Quill needs one to assemble and link programs: install GCC or Clang \
         (on Windows, MSYS2's mingw-w64-ucrt-x86_64-gcc), or set QUILL_CC",
    );
    for cc in candidates() {
        match runtime_object(&cc) {
            Ok(_) => return Ok(cc),
            Err(e) => msg.push_str(&format!("\n  {} does not work: {}", cc.display(), e.lines().nth(1).unwrap_or(&e))),
        }
    }
    Err(msg)
}

fn candidates() -> Vec<PathBuf> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let on_path = |name: &str| -> Vec<PathBuf> { dirs.iter().map(|d| d.join(format!("{name}{exe}"))).filter(|p| p.is_file()).collect() };
    let mut out = on_path("gcc");
    if cfg!(windows) {
        let msys = [r"C:\msys64\ucrt64\bin\gcc.exe", r"C:\msys64\mingw64\bin\gcc.exe", r"C:\msys64\clang64\bin\clang.exe"];
        out.extend(msys.into_iter().map(PathBuf::from).filter(|p| p.is_file()));
    }
    out.extend(on_path("clang"));
    out.extend(on_path("cc"));
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

fn command_for(cc: &Path) -> Command {
    let mut cmd = Command::new(cc);
    // MSYS2's gcc needs its own bin directory on the PATH to find the assembler and linker DLLs.
    if let Some(dir) = cc.parent()
        && let Some(path) = std::env::var_os("PATH")
    {
        let mut paths: Vec<PathBuf> = vec![dir.to_path_buf()];
        paths.extend(std::env::split_paths(&path));
        if let Ok(joined) = std::env::join_paths(paths) {
            cmd.env("PATH", joined);
        }
    }
    cmd
}

fn run(mut cmd: Command, what: &str) -> Result<(), String> {
    let out = cmd.output().map_err(|e| format!("could not run the C compiler for {what}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{what} failed:\n{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    }
    Ok(())
}

/// The compiled run-time library, built on first use and cached by source hash.
pub fn runtime_object(cc: &Path) -> Result<PathBuf, String> {
    let mut h = DefaultHasher::new();
    RUNTIME_SOURCE.hash(&mut h);
    cc.hash(&mut h);
    let key = h.finish();
    let dir = std::env::temp_dir().join("quill-cache");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let obj = dir.join(format!("quill_rt-{key:016x}.o"));
    if obj.is_file() {
        return Ok(obj);
    }
    let failed = dir.join(format!("quill_rt-{key:016x}.failed"));
    if let Ok(e) = std::fs::read_to_string(&failed) {
        return Err(e);
    }
    let src = dir.join(format!("quill_rt-{key:016x}.c"));
    std::fs::write(&src, RUNTIME_SOURCE).map_err(|e| format!("cannot write {}: {e}", src.display()))?;
    // Build to a unique name, then rename, so concurrent builds do not trip over each other.
    let tmp = dir.join(format!("quill_rt-{key:016x}-{}.o", std::process::id()));
    let mut cmd = command_for(cc);
    cmd.args(["-std=gnu11", "-O2", "-c"]).arg(&src).arg("-o").arg(&tmp);
    if let Err(e) = run(cmd, "compiling the run-time library") {
        let _ = std::fs::write(&failed, &e);
        return Err(e);
    }
    let _ = std::fs::rename(&tmp, &obj);
    if !obj.is_file() {
        return Ok(tmp);
    }
    Ok(obj)
}

/// Assembles `asm` and links it with the run-time library into `output`.
pub fn link(asm: &str, output: &Path) -> Result<(), String> {
    let cc = find_cc()?;
    let rt = runtime_object(&cc)?;
    let asm_path = output.with_extension("s");
    std::fs::write(&asm_path, asm).map_err(|e| format!("cannot write {}: {e}", asm_path.display()))?;
    let mut cmd = command_for(&cc);
    cmd.arg(&asm_path).arg(&rt).arg("-o").arg(output);
    if !cfg!(windows) {
        cmd.arg("-pthread");
    }

    run(cmd, "assembling and linking")
}
