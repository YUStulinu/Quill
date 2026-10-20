// VS Code client for the Quill language server, plus commands to run a file and look at what the
// compiler makes of it.

import * as cp from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node";

let client: LanguageClient | undefined;

/** The quill executable: the setting, then a build in the open workspace, then the PATH. */
function quillPath(): string {
  const configured = vscode.workspace.getConfiguration("quill").get<string>("path");
  if (configured) {
    // VS Code does not expand variables in extension settings; support the common one.
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? "";
    return configured.replace("${workspaceFolder}", root);
  }
  const exe = process.platform === "win32" ? "quill.exe" : "quill";
  for (const folder of vscode.workspace.workspaceFolders ?? []) {
    for (const profile of ["release", "debug"]) {
      const candidate = path.join(folder.uri.fsPath, "target", profile, exe);
      if (fs.existsSync(candidate)) {
        return candidate;
      }
    }
  }
  return "quill";
}

function startClient(context: vscode.ExtensionContext) {
  const command = quillPath();
  const serverOptions: ServerOptions = { run: { command, args: ["lsp"] }, debug: { command, args: ["lsp"] } };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "quill" }, { scheme: "untitled", language: "quill" }],
  };
  client = new LanguageClient("quill", "Quill Language Server", serverOptions, clientOptions);
  client.start().catch((err) => {
    vscode.window.showErrorMessage(
      `Could not start the Quill language server (${command}): ${err}. Build it with \`cargo build --release\` or set "quill.path".`
    );
  });
  context.subscriptions.push({ dispose: () => client?.stop() });
}

function currentQuillFile(): vscode.TextDocument | undefined {
  const doc = vscode.window.activeTextEditor?.document;
  if (!doc || doc.languageId !== "quill") {
    vscode.window.showWarningMessage("Open a .ql file first.");
    return undefined;
  }
  return doc;
}

async function runFile() {
  const doc = currentQuillFile();
  if (!doc) {
    return;
  }
  await doc.save();
  const level = vscode.workspace.getConfiguration("quill").get<string>("optLevel") ?? "-O2";
  let terminal = vscode.window.terminals.find((t) => t.name === "Quill");
  if (!terminal) {
    terminal = vscode.window.createTerminal("Quill");
  }
  terminal.show(true);
  const quote = (s: string) => (s.includes(" ") ? `"${s}"` : s);
  const prefix = process.platform === "win32" && quillPath().includes(" ") ? "& " : "";
  terminal.sendText(`${prefix}${quote(quillPath())} run ${quote(doc.fileName)} ${level}`);
}

async function showOutput(emit: "ir-opt" | "asm") {
  const doc = currentQuillFile();
  if (!doc) {
    return;
  }
  await doc.save();
  const level = vscode.workspace.getConfiguration("quill").get<string>("optLevel") ?? "-O2";
  cp.execFile(quillPath(), ["build", doc.fileName, "--emit", emit, level], { maxBuffer: 64 * 1024 * 1024 }, async (err, stdout, stderr) => {
    if (err && !stdout) {
      vscode.window.showErrorMessage(`quill: ${stderr || err.message}`);
      return;
    }
    const out = await vscode.workspace.openTextDocument({ content: stdout, language: emit === "asm" ? "asm" : "plaintext" });
    await vscode.window.showTextDocument(out, { viewColumn: vscode.ViewColumn.Beside, preview: true });
  });
}

export function activate(context: vscode.ExtensionContext) {
  startClient(context);
  context.subscriptions.push(
    vscode.commands.registerCommand("quill.run", runFile),
    vscode.commands.registerCommand("quill.showIr", () => showOutput("ir-opt")),
    vscode.commands.registerCommand("quill.showAsm", () => showOutput("asm")),
    vscode.commands.registerCommand("quill.restartServer", async () => {
      await client?.stop();
      startClient(context);
    })
  );
}

export function deactivate(): Thenable<void> | undefined {
  return client?.stop();
}
