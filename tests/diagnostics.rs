//! Error-message tests: every program in `tests/errors` must produce exactly the errors and warnings
//! listed in its `// error:` and `// warning:` comments (each comment is a substring of one message,
//! label or note of a diagnostic of that severity), and no other errors.

use std::path::Path;

use quill::diag::Severity;

#[test]
fn expected_diagnostics() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("errors");
    let mut failures = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    files.sort();
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let checked = quill::check_source(&name, &src);
        let text = |d: &quill::diag::Diagnostic| {
            let mut t = d.message.clone();
            if let Some(l) = &d.label {
                t.push_str(" | ");
                t.push_str(l);
            }
            for n in &d.notes {
                t.push_str(" | ");
                t.push_str(n);
            }
            t
        };
        for (marker, severity) in [("// error: ", Severity::Error), ("// warning: ", Severity::Warning)] {
            let want: Vec<&str> = src.lines().filter_map(|l| l.trim().strip_prefix(marker)).collect();
            let got: Vec<String> = checked
                .diags
                .list
                .iter()
                .filter(|d| d.severity == severity)
                .filter(|d| severity == Severity::Error || !d.message.starts_with("unused"))
                .map(text)
                .collect();
            for w in &want {
                if !got.iter().any(|g| g.contains(w)) {
                    failures.push(format!("{name}: expected {marker}{w}\n  got: {got:#?}"));
                }
            }
            if severity == Severity::Error && got.len() != want.len() {
                failures.push(format!("{name}: expected {} errors, got {}: {got:#?}", want.len(), got.len()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn diagnostics_render_with_source_and_carets() {
    let checked = quill::check_source("demo.ql", "fn main() {\n  let x = 1 + \"two\";\n}\n");
    let r = quill::diag::Renderer { color: false };
    let d = checked.diags.list.iter().find(|d| d.is_error()).unwrap();
    let out = r.render(&checked.file, d);
    assert!(out.contains("--> demo.ql:2:15"), "{out}");
    assert!(out.contains("let x = 1 + \"two\";"), "{out}");
    assert!(out.contains("^^^^^ this is a String"), "{out}");
}
