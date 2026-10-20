//! The language server: diagnostics as you type, the inferred type on hover, go to definition,
//! completion, document symbols, and inlay hints showing inferred types of bindings.
//!
//! It speaks LSP over standard input/output (`quill lsp`), and re-checks a document on every change: the
//! front end is fast enough for that, and the parser recovers from errors so a half-written line does not
//! hide the rest of the file.

use std::collections::HashMap;

use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::{DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics};
use lsp_types::request::{Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, InlayHintRequest, Request as _};
use lsp_types::*;

use crate::ast::{ExprKind, Item, PatKind, Stmt};
use crate::builtins;
use crate::diag::{Severity, SourceFile, Span};
use crate::typeck::{Res, Typed, walk_expr};
use crate::types::{TyPrinter, show};

struct Doc {
    text: String,
    file: SourceFile,
    typed: Typed,
}

// `Uri` caches its parsed form internally, which clippy mistakes for a mutable hash key.
#[allow(clippy::mutable_key_type)]
pub fn run() -> Result<(), String> {
    let (conn, io) = Connection::stdio();
    let caps = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions::default()),
        document_symbol_provider: Some(OneOf::Left(true)),
        inlay_hint_provider: Some(OneOf::Left(true)),
        ..Default::default()
    };
    let caps = serde_json::to_value(caps).map_err(|e| e.to_string())?;
    conn.initialize(caps).map_err(|e| e.to_string())?;
    let mut docs: HashMap<Uri, Doc> = HashMap::new();
    for msg in &conn.receiver {
        match msg {
            Message::Request(req) => {
                if conn.handle_shutdown(&req).map_err(|e| e.to_string())? {
                    break;
                }
                let resp = handle_request(&docs, req);
                conn.sender.send(Message::Response(resp)).map_err(|e| e.to_string())?;
            }
            Message::Notification(n) => {
                if let Some((uri, diags)) = handle_notification(&mut docs, n) {
                    let params = PublishDiagnosticsParams { uri, diagnostics: diags, version: None };
                    let note = Notification::new(PublishDiagnostics::METHOD.to_string(), params);
                    conn.sender.send(Message::Notification(note)).map_err(|e| e.to_string())?;
                }
            }
            Message::Response(_) => {}
        }
    }
    // The reader thread blocks on stdin until the client closes it; there is nothing left to wait for.
    drop(conn);
    drop(io);
    Ok(())
}

fn analyze(uri: &Uri, text: String) -> (Doc, Vec<Diagnostic>) {
    let name = uri.path().as_str().rsplit('/').next().unwrap_or("file.ql").to_string();
    let checked = crate::check_source(&name, &text);
    let diags = checked
        .diags
        .list
        .iter()
        // "No main" is noise while a library-like file is being written.
        .filter(|d| !(d.message == "no `main` function" && !text.contains("fn main")))
        .map(|d| {
            let mut message = d.message.clone();
            if let Some(l) = &d.label
                && !l.is_empty()
            {
                message.push_str(&format!(" ({l})"));
            }
            for n in &d.notes {
                message.push_str(&format!("\n{n}"));
            }
            Diagnostic {
                range: range(&checked.file, d.span),
                severity: Some(match d.severity {
                    Severity::Error => DiagnosticSeverity::ERROR,
                    Severity::Warning => DiagnosticSeverity::WARNING,
                }),
                source: Some("quill".into()),
                message,
                related_information: Some(
                    d.secondary
                        .iter()
                        .map(|l| DiagnosticRelatedInformation {
                            location: Location { uri: uri.clone(), range: range(&checked.file, l.span) },
                            message: l.message.clone(),
                        })
                        .collect(),
                ),
                tags: if d.message.starts_with("unused") || d.message.starts_with("unreachable") {
                    Some(vec![DiagnosticTag::UNNECESSARY])
                } else {
                    None
                },
                ..Default::default()
            }
        })
        .collect();
    (Doc { text, file: checked.file, typed: checked.typed }, diags)
}

#[allow(clippy::mutable_key_type)]
fn handle_notification(docs: &mut HashMap<Uri, Doc>, n: Notification) -> Option<(Uri, Vec<Diagnostic>)> {
    match n.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let p: DidOpenTextDocumentParams = serde_json::from_value(n.params).ok()?;
            let (doc, diags) = analyze(&p.text_document.uri, p.text_document.text);
            docs.insert(p.text_document.uri.clone(), doc);
            Some((p.text_document.uri, diags))
        }
        DidChangeTextDocument::METHOD => {
            let p: DidChangeTextDocumentParams = serde_json::from_value(n.params).ok()?;
            let text = p.content_changes.into_iter().last()?.text;
            let (doc, diags) = analyze(&p.text_document.uri, text);
            docs.insert(p.text_document.uri.clone(), doc);
            Some((p.text_document.uri, diags))
        }
        DidCloseTextDocument::METHOD => {
            let p: DidCloseTextDocumentParams = serde_json::from_value(n.params).ok()?;
            docs.remove(&p.text_document.uri);
            Some((p.text_document.uri, vec![]))
        }
        _ => None,
    }
}

#[allow(clippy::mutable_key_type)]
fn handle_request(docs: &HashMap<Uri, Doc>, req: Request) -> Response {
    let id = req.id.clone();
    let result = match req.method.as_str() {
        HoverRequest::METHOD => params::<HoverParams>(&req).and_then(|p| {
            let d = docs.get(&p.text_document_position_params.text_document.uri)?;
            serde_json::to_value(hover(d, p.text_document_position_params.position)).ok()
        }),
        GotoDefinition::METHOD => params::<GotoDefinitionParams>(&req).and_then(|p| {
            let uri = p.text_document_position_params.text_document.uri.clone();
            let d = docs.get(&uri)?;
            let span = definition(d, p.text_document_position_params.position)?;
            serde_json::to_value(GotoDefinitionResponse::Scalar(Location { uri, range: range(&d.file, span) })).ok()
        }),
        Completion::METHOD => params::<CompletionParams>(&req).and_then(|p| {
            let d = docs.get(&p.text_document_position.text_document.uri)?;
            serde_json::to_value(CompletionResponse::Array(completions(d, p.text_document_position.position))).ok()
        }),
        DocumentSymbolRequest::METHOD => params::<DocumentSymbolParams>(&req).and_then(|p| {
            let d = docs.get(&p.text_document.uri)?;
            serde_json::to_value(DocumentSymbolResponse::Nested(symbols(d))).ok()
        }),
        InlayHintRequest::METHOD => params::<InlayHintParams>(&req).and_then(|p| {
            let d = docs.get(&p.text_document.uri)?;
            serde_json::to_value(inlay_hints(d)).ok()
        }),
        _ => None,
    };
    respond(id, result)
}

fn params<P: serde::de::DeserializeOwned>(req: &Request) -> Option<P> {
    serde_json::from_value(req.params.clone()).ok()
}

fn respond(id: RequestId, result: Option<serde_json::Value>) -> Response {
    Response::new_ok(id, result.unwrap_or(serde_json::Value::Null))
}

fn range(f: &SourceFile, s: Span) -> Range {
    let (l1, c1) = f.utf16_pos(s.start as usize);
    let (l2, c2) = f.utf16_pos(s.end as usize);
    Range { start: Position { line: l1 as u32, character: c1 as u32 }, end: Position { line: l2 as u32, character: c2 as u32 } }
}

fn offset(d: &Doc, p: Position) -> usize {
    d.file.offset_of_utf16(p.line as usize, p.character as usize)
}

/// The innermost name, binding or parameter at an offset.
fn node_at(d: &Doc, off: usize) -> Option<(u32, Span)> {
    d.typed.spans.iter().filter(|(_, s)| s.contains(off)).min_by_key(|(_, s)| s.len()).map(|(&id, &s)| (id, s))
}

fn code_block(s: &str) -> MarkupContent {
    MarkupContent { kind: MarkupKind::Markdown, value: format!("```quill\n{s}\n```") }
}

fn hover(d: &Doc, p: Position) -> Option<Hover> {
    let off = offset(d, p);
    let (id, span) = node_at(d, off)?;
    let name = &d.text[span.start as usize..span.end as usize];
    let mut extra = String::new();
    let text = match d.typed.res.get(&id) {
        Some(Res::Global(f)) => format!("fn {f} : {}", show(&d.typed.fns.get(f)?.scheme.ty)),
        Some(Res::Builtin(b)) => {
            let info = builtins::info(*b);
            extra = format!("\n\n{}", info.doc);
            format!("{} : {}", info.name, show(&builtins::scheme(*b).ty))
        }
        Some(Res::Ctor(a, t)) => {
            let adt = &d.typed.adts[*a];
            let ctor = &adt.ctors[*t as usize];
            let mut p = TyPrinter::default();
            let params: Vec<String> = (0..adt.params.len()).map(|i| p.print(&crate::types::Ty::Gen(i as u32))).collect();
            let result = if params.is_empty() { adt.name.clone() } else { format!("{}[{}]", adt.name, params.join(", ")) };
            if ctor.fields.is_empty() {
                format!("{} : {result}", ctor.name)
            } else {
                let fields: Vec<String> = ctor.fields.iter().map(|f| p.print(f)).collect();
                format!("{}({}) : {result}", ctor.name, fields.join(", "))
            }
        }
        Some(Res::Local(b)) => {
            let bind = &d.typed.bindings[*b as usize];
            let ty = bind.scheme.as_ref().map_or_else(|| show(&bind.ty), |s| show(&s.ty));
            format!("{}{} : {ty}", if bind.mutable { "var " } else { "" }, bind.name)
        }
        None => match d.typed.pat_bind.get(&id) {
            Some(&b) => {
                let bind = &d.typed.bindings[b as usize];
                format!("{}{} : {}", if bind.mutable { "var " } else { "" }, bind.name, show(&bind.ty))
            }
            None => format!("{name} : {}", show(&d.typed.types.get(&id)?.clone())),
        },
    };
    let mut content = code_block(&text);
    content.value.push_str(&extra);
    Some(Hover { contents: HoverContents::Markup(content), range: Some(range(&d.file, span)) })
}

fn definition(d: &Doc, p: Position) -> Option<Span> {
    let (id, _) = node_at(d, offset(d, p))?;
    match d.typed.res.get(&id)? {
        Res::Local(b) => Some(d.typed.bindings[*b as usize].span),
        Res::Global(f) => Some(d.typed.fns.get(f)?.name_span),
        Res::Ctor(a, t) => Some(d.typed.adts[*a].ctors[*t as usize].span),
        Res::Builtin(_) => None,
    }
}

fn completions(d: &Doc, p: Position) -> Vec<CompletionItem> {
    let off = offset(d, p);
    let mut items = Vec::new();
    for kw in crate::lexer::KEYWORDS {
        items.push(CompletionItem { label: kw.to_string(), kind: Some(CompletionItemKind::KEYWORD), ..Default::default() });
    }
    for b in builtins::BUILTINS.iter().filter(|b| b.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
        items.push(CompletionItem {
            label: b.name.to_string(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(show(&builtins::scheme(b.builtin).ty)),
            documentation: Some(Documentation::String(b.doc.to_string())),
            ..Default::default()
        });
    }
    for (name, f) in &d.typed.fns {
        items.push(CompletionItem {
            label: name.clone(),
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(show(&f.scheme.ty)),
            ..Default::default()
        });
    }
    for adt in &d.typed.adts {
        items.push(CompletionItem { label: adt.name.clone(), kind: Some(CompletionItemKind::ENUM), ..Default::default() });
        for c in &adt.ctors {
            items.push(CompletionItem {
                label: c.name.clone(),
                kind: Some(CompletionItemKind::ENUM_MEMBER),
                detail: Some(adt.name.clone()),
                ..Default::default()
            });
        }
    }
    // Local bindings of the function the cursor is in, declared before the cursor.
    let enclosing = d.typed.program.items.iter().find_map(|i| match i {
        Item::Fn(f) if f.span.contains(off) => Some(f.span),
        _ => None,
    });
    if let Some(fs) = enclosing {
        let mut seen = std::collections::HashSet::new();
        for b in &d.typed.bindings {
            if fs.contains(b.span.start as usize) && (b.span.end as usize) <= off && b.name != "_" && seen.insert(b.name.clone()) {
                items.push(CompletionItem {
                    label: b.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(show(&b.ty)),
                    ..Default::default()
                });
            }
        }
    }
    items
}

#[allow(deprecated)]
fn symbols(d: &Doc) -> Vec<DocumentSymbol> {
    let mut out = Vec::new();
    for item in &d.typed.program.items {
        match item {
            Item::Fn(f) => out.push(DocumentSymbol {
                name: f.name.name.clone(),
                detail: d.typed.fns.get(&f.name.name).map(|i| show(&i.scheme.ty)),
                kind: SymbolKind::FUNCTION,
                tags: None,
                deprecated: None,
                range: range(&d.file, f.span),
                selection_range: range(&d.file, f.name.span),
                children: None,
            }),
            Item::Type(t) => out.push(DocumentSymbol {
                name: t.name.name.clone(),
                detail: None,
                kind: SymbolKind::ENUM,
                tags: None,
                deprecated: None,
                range: range(&d.file, t.span),
                selection_range: range(&d.file, t.name.span),
                children: Some(
                    t.ctors
                        .iter()
                        .map(|c| DocumentSymbol {
                            name: c.name.name.clone(),
                            detail: None,
                            kind: SymbolKind::ENUM_MEMBER,
                            tags: None,
                            deprecated: None,
                            range: range(&d.file, c.name.span),
                            selection_range: range(&d.file, c.name.span),
                            children: None,
                        })
                        .collect(),
                ),
            }),
        }
    }
    out
}

/// `: Type` after every `let`/`var` name and every unannotated function parameter.
fn inlay_hints(d: &Doc) -> Vec<InlayHint> {
    let mut hints = Vec::new();
    let push = |span: Span, id: u32, hints: &mut Vec<InlayHint>| {
        if let Some(&b) = d.typed.pat_bind.get(&id) {
            let bind = &d.typed.bindings[b as usize];
            let ty = bind.scheme.as_ref().map_or_else(|| show(&bind.ty), |s| show(&s.ty));
            let (line, col) = d.file.utf16_pos(span.end as usize);
            hints.push(InlayHint {
                position: Position { line: line as u32, character: col as u32 },
                label: InlayHintLabel::String(format!(": {ty}")),
                kind: Some(InlayHintKind::TYPE),
                text_edits: None,
                tooltip: None,
                padding_left: None,
                padding_right: None,
                data: None,
            });
        }
    };
    for item in &d.typed.program.items {
        let Item::Fn(f) = item else { continue };
        for p in &f.params {
            if p.ty.is_none() && p.name.name != "_" {
                push(p.name.span, p.id, &mut hints);
            }
        }
        walk_expr(&f.body, &mut |e| {
            if let ExprKind::Block(stmts, _) = &e.kind {
                for s in stmts {
                    if let Stmt::Let { pat, ty: None, .. } = s
                        && let PatKind::Bind(name) = &pat.kind
                        && name != "_"
                    {
                        push(pat.span, pat.id, &mut hints);
                    }
                }
            }
        });
    }
    hints
}
