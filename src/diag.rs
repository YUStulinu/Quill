//! Source spans and diagnostics, rendered with the offending source line and a caret underline.

use std::fmt::Write as _;

/// A byte range in a source file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start: start as u32, end: end as u32 }
    }

    pub fn to(self, other: Span) -> Span {
        Span { start: self.start.min(other.start), end: self.end.max(other.end) }
    }

    pub fn contains(self, offset: usize) -> bool {
        (self.start as usize) <= offset && offset <= (self.end as usize)
    }

    pub fn len(self) -> usize {
        (self.end - self.start) as usize
    }

    pub fn is_empty(self) -> bool {
        self.end <= self.start
    }
}

/// A source file with a line index, for turning byte offsets into line/column positions.
#[derive(Clone, Debug)]
pub struct SourceFile {
    pub name: String,
    pub text: String,
    line_starts: Vec<usize>,
}

impl SourceFile {
    pub fn new(name: impl Into<String>, text: impl Into<String>) -> SourceFile {
        let text = text.into();
        let mut line_starts = vec![0];
        for (i, b) in text.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        SourceFile { name: name.into(), text, line_starts }
    }

    /// Zero-based (line, column in characters) of a byte offset.
    pub fn line_col(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line = match self.line_starts.binary_search(&offset) {
            Ok(l) => l,
            Err(l) => l - 1,
        };
        let start = self.line_starts[line];
        let col = self.text[start..offset].chars().count();
        (line, col)
    }

    /// Byte offset of a zero-based (line, UTF-16 column) position, as LSP clients send them.
    pub fn offset_of_utf16(&self, line: usize, col16: usize) -> usize {
        if line >= self.line_starts.len() {
            return self.text.len();
        }
        let start = self.line_starts[line];
        let end = self.line_starts.get(line + 1).copied().unwrap_or(self.text.len());
        let mut units = 0;
        for (i, ch) in self.text[start..end].char_indices() {
            if units >= col16 {
                return start + i;
            }
            units += ch.len_utf16();
        }
        end
    }

    /// Zero-based (line, UTF-16 column) of a byte offset.
    pub fn utf16_pos(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let (line, _) = self.line_col(offset);
        let start = self.line_starts[line];
        let col = self.text[start..offset].chars().map(char::len_utf16).sum();
        (line, col)
    }

    pub fn line_text(&self, line: usize) -> &str {
        let start = self.line_starts[line];
        let end = self.line_starts.get(line + 1).copied().unwrap_or(self.text.len());
        self.text[start..end].trim_end_matches(['\n', '\r'])
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

/// A secondary location attached to a diagnostic ("first defined here").
#[derive(Clone, Debug)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub span: Span,
    /// Text shown under the caret.
    pub label: Option<String>,
    pub secondary: Vec<Label>,
    pub notes: Vec<String>,
}

impl Diagnostic {
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Error, message: message.into(), span, label: None, secondary: vec![], notes: vec![] }
    }

    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        Diagnostic { severity: Severity::Warning, message: message.into(), span, label: None, secondary: vec![], notes: vec![] }
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Diagnostic {
        self.label = Some(label.into());
        self
    }

    pub fn with_secondary(mut self, span: Span, message: impl Into<String>) -> Diagnostic {
        self.secondary.push(Label { span, message: message.into() });
        self
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        self
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Renders diagnostics in the familiar compiler style:
///
/// ```text
/// error: mismatched types: expected Int, found String
///   --> examples/demo.ql:3:13
///    |
///  3 |     let x = 1 + "two";
///    |                 ^^^^^ this is a String
/// ```
pub struct Renderer {
    pub color: bool,
}

impl Renderer {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color { format!("\x1b[{code}m{text}\x1b[0m") } else { text.to_string() }
    }

    pub fn render(&self, file: &SourceFile, d: &Diagnostic) -> String {
        let mut out = String::new();
        let (kind, color) = match d.severity {
            Severity::Error => ("error", "1;31"),
            Severity::Warning => ("warning", "1;33"),
        };
        let _ = writeln!(out, "{}{}", self.paint(color, kind), self.paint("1", &format!(": {}", d.message)));
        let (line, col) = file.line_col(d.span.start as usize);
        let gutter = format!("{}", file.line_count()).len().max(2);
        let _ = writeln!(out, "{:>gutter$}{} {}:{}:{}", "", self.paint("1;34", "-->"), file.name, line + 1, col + 1, gutter = gutter);
        self.snippet(&mut out, file, d.span, d.label.as_deref().unwrap_or(""), color, gutter);
        for l in &d.secondary {
            self.snippet(&mut out, file, l.span, &l.message, "1;34", gutter);
        }
        for n in &d.notes {
            let _ = writeln!(out, "{:>gutter$} {} {}", "", self.paint("1;34", "="), n, gutter = gutter);
        }
        out
    }

    fn snippet(&self, out: &mut String, file: &SourceFile, span: Span, label: &str, color: &str, gutter: usize) {
        let (line, col) = file.line_col(span.start as usize);
        let (end_line, end_col) = file.line_col(span.end as usize);
        let text = file.line_text(line);
        let bar = self.paint("1;34", "|");
        let _ = writeln!(out, "{:>gutter$} {}", "", bar, gutter = gutter);
        let _ = writeln!(out, "{} {} {}", self.paint("1;34", &format!("{:>gutter$}", line + 1, gutter = gutter)), bar, text);
        let width = if end_line == line { (end_col.saturating_sub(col)).max(1) } else { text.chars().count().saturating_sub(col).max(1) };
        let carets = "^".repeat(width);
        let _ = writeln!(
            out,
            "{:>gutter$} {} {}{} {}",
            "",
            bar,
            " ".repeat(col),
            self.paint(color, &carets),
            self.paint(color, label),
            gutter = gutter
        );
    }
}

/// Collects diagnostics during a compilation.
#[derive(Default, Debug, Clone)]
pub struct Diagnostics {
    pub list: Vec<Diagnostic>,
}

impl Diagnostics {
    pub fn push(&mut self, d: Diagnostic) {
        self.list.push(d);
    }

    pub fn error(&mut self, span: Span, message: impl Into<String>) {
        self.push(Diagnostic::error(span, message));
    }

    pub fn has_errors(&self) -> bool {
        self.list.iter().any(Diagnostic::is_error)
    }

    pub fn extend(&mut self, other: Diagnostics) {
        self.list.extend(other.list);
    }
}
