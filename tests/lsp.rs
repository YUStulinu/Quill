//! The language server, end to end: starts `quill lsp` and talks JSON-RPC to it like an editor would.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{Value, json};

struct Server {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start() -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_quill")).arg("lsp").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server { child, stdin, stdout }
    }

    fn send(&mut self, msg: Value) {
        let body = msg.to_string();
        write!(self.stdin, "Content-Length: {}\r\n\r\n{body}", body.len()).unwrap();
        self.stdin.flush().unwrap();
    }

    fn recv(&mut self) -> Value {
        let mut len = 0;
        loop {
            let mut line = String::new();
            self.stdout.read_line(&mut line).unwrap();
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some(v) = line.strip_prefix("Content-Length: ") {
                len = v.parse().unwrap();
            }
        }
        let mut buf = vec![0; len];
        self.stdout.read_exact(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let m = self.recv();
            if m["id"] == json!(id) {
                return m["result"].clone();
            }
        }
    }
}

const URI: &str = "file:///tmp/demo.ql";
const TEXT: &str = "type List[a] = Nil | Cons(a, List[a])
fn map(xs, f) {
  match xs { Nil => Nil, Cons(x, rest) => Cons(f(x), map(rest, f)) }
}
fn main() {
  let doubled = map(Cons(1, Nil), |x| x * 2);
  let bad = 1 + \"two\";
  println(int_to_string(0));
}
";

#[test]
fn language_server_features() {
    let mut s = Server::start();
    s.request(1, "initialize", json!({"processId": null, "rootUri": null, "capabilities": {}}));
    s.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
    s.send(json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {"uri": URI, "languageId": "quill", "version": 1, "text": TEXT}}}));

    // Diagnostics arrive as a notification.
    let diags = s.recv();
    assert_eq!(diags["method"], "textDocument/publishDiagnostics");
    let list = diags["params"]["diagnostics"].as_array().unwrap();
    assert!(list.iter().any(|d| d["message"].as_str().unwrap().contains("`+` needs Int operands") && d["severity"] == 1));
    assert!(list.iter().any(|d| d["message"].as_str().unwrap().contains("unused variable `bad`") && d["severity"] == 2));

    // Hover: the inferred polymorphic type of map, and of a local.
    let h = s.request(2, "textDocument/hover", json!({"textDocument": {"uri": URI}, "position": {"line": 5, "character": 16}}));
    assert!(h["contents"]["value"].as_str().unwrap().contains("fn map : (List[a], (a) -> b) -> List[b]"), "{h}");
    let h = s.request(3, "textDocument/hover", json!({"textDocument": {"uri": URI}, "position": {"line": 5, "character": 7}}));
    assert!(h["contents"]["value"].as_str().unwrap().contains("doubled : List[Int]"), "{h}");

    // Go to definition of map: line 1, column 3.
    let d = s.request(4, "textDocument/definition", json!({"textDocument": {"uri": URI}, "position": {"line": 5, "character": 16}}));
    assert_eq!(d["range"]["start"], json!({"line": 1, "character": 3}));

    // Completion offers functions, constructors, builtins and locals in scope.
    let c = s.request(5, "textDocument/completion", json!({"textDocument": {"uri": URI}, "position": {"line": 7, "character": 2}}));
    let labels: Vec<&str> = c.as_array().unwrap().iter().map(|i| i["label"].as_str().unwrap()).collect();
    for want in ["map", "Cons", "println", "doubled", "match"] {
        assert!(labels.contains(&want), "missing {want} in {labels:?}");
    }

    // Inlay hints show inferred types.
    let ih = s.request(
        6,
        "textDocument/inlayHint",
        json!({"textDocument": {"uri": URI}, "range": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}}}),
    );
    let hints: Vec<&str> = ih.as_array().unwrap().iter().map(|h| h["label"].as_str().unwrap()).collect();
    assert!(hints.contains(&": List[Int]"), "{hints:?}");

    // Fixing the error clears it.
    let fixed = TEXT.replace("1 + \"two\"", "1 + 2");
    s.send(json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {"textDocument": {"uri": URI, "version": 2}, "contentChanges": [{"text": fixed}]}}));
    let diags = s.recv();
    assert!(diags["params"]["diagnostics"].as_array().unwrap().iter().all(|d| d["severity"] != 1));

    s.request(7, "shutdown", Value::Null);
    s.send(json!({"jsonrpc": "2.0", "method": "exit", "params": null}));
    drop(s.stdin);
    let status = s.child.wait().unwrap();
    assert!(status.success());
}
