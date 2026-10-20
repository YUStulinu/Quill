"""Builds and times the benchmarks: Quill at -O0 and -O2, the same programs in Python, and two of them in C.

Usage: python bench/run.py [--runs N] [--quill path/to/quill]
Prints a Markdown table (best of N runs, wall-clock seconds) and checks that every version prints the same.
"""

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
EXE = ".exe" if os.name == "nt" else ""
BENCHES = ["fib", "loops", "sieve", "binary_trees", "closures", "strings"]
C_VERSIONS = {"fib", "loops"}


def best_time(cmd, runs):
    best, out = None, None
    for _ in range(runs):
        t0 = time.perf_counter()
        r = subprocess.run(cmd, capture_output=True, text=True)
        dt = time.perf_counter() - t0
        if r.returncode != 0:
            raise SystemExit(f"{' '.join(cmd)} failed:\n{r.stderr}")
        best = dt if best is None else min(best, dt)
        out = r.stdout
    return best, out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--quill", default=os.path.join(ROOT, "target", "release", "quill" + EXE))
    args = ap.parse_args()
    if not os.path.exists(args.quill):
        raise SystemExit(f"{args.quill} not found: run `cargo build --release` first")
    cc = shutil.which("gcc") or shutil.which("clang") or shutil.which("cc")
    work = tempfile.mkdtemp(prefix="quill-bench-")
    rows = []
    for b in BENCHES:
        src = os.path.join(HERE, f"{b}.ql")
        times = {}
        outputs = {}
        for level in ["-O0", "-O2"]:
            exe = os.path.join(work, f"{b}{level}{EXE}")
            subprocess.run([args.quill, "build", src, level, "-o", exe], check=True, capture_output=True)
            times[level], outputs[level] = best_time([exe], args.runs)
        py = os.path.join(HERE, "python", f"{b}.py")
        times["python"], outputs["python"] = best_time([sys.executable, py], max(1, args.runs // 2))
        if b in C_VERSIONS and cc:
            exe = os.path.join(work, f"{b}-c{EXE}")
            subprocess.run([cc, "-O2", "-o", exe, os.path.join(HERE, "c", f"{b}.c")], check=True)
            times["c"], outputs["c"] = best_time([exe], args.runs)
        same = len({o.replace("\r\n", "\n") for o in outputs.values()}) == 1
        rows.append((b, times, same))
    print("| benchmark | Quill -O0 | Quill -O2 | speed-up | Python | Quill vs Python | C (gcc -O2) |")
    print("|---|---:|---:|---:|---:|---:|---:|")
    for b, t, same in rows:
        c = f"{t['c']:.3f} s" if "c" in t else "-"
        flag = "" if same else " (outputs differ!)"
        print(f"| {b}{flag} | {t['-O0']:.3f} s | {t['-O2']:.3f} s | {t['-O0'] / t['-O2']:.1f}x | {t['python']:.2f} s | {t['python'] / t['-O2']:.0f}x faster | {c} |")
    shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
