import * as vscode from "vscode";
import * as path from "path";

export async function activate(): Promise<vscode.Extension<any>> {
  // In development mode the publisher may be missing, so find the extension
  // by matching on the package name rather than a hardcoded qualified ID.
  const ext = vscode.extensions.all.find((e) => e.id.endsWith("br-lsp"));
  if (!ext) {
    throw new Error("br-lsp extension not found among installed extensions");
  }
  if (!ext.isActive) {
    await ext.activate();
  }
  return ext;
}

export function getFixturePath(fileName: string): string {
  // testFixture is the workspace root opened by the test runner
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  if (workspaceFolder) {
    return path.join(workspaceFolder.uri.fsPath, fileName);
  }
  // Fallback: resolve relative to this file's location
  return path.resolve(__dirname, "../../../testFixture", fileName);
}

const processedUris = new Set<string>();

export async function openDocument(fileName: string): Promise<vscode.TextDocument> {
  const uri = vscode.Uri.file(getFixturePath(fileName));
  const uriStr = uri.toString();

  // For the first open of a file, set up a diagnostics listener BEFORE
  // opening so we catch the server's publishDiagnostics response to didOpen.
  let serverReady: Promise<void> | undefined;
  if (!processedUris.has(uriStr)) {
    processedUris.add(uriStr);
    serverReady = new Promise<void>((resolve) => {
      const timer = setTimeout(() => {
        disposable.dispose();
        resolve();
      }, 2000);
      const disposable = vscode.languages.onDidChangeDiagnostics((e) => {
        if (e.uris.find((u) => u.toString() === uriStr)) {
          clearTimeout(timer);
          disposable.dispose();
          resolve();
        }
      });
    });
  }

  const doc = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(doc);

  if (serverReady) {
    await serverReady;
  }

  return doc;
}

export async function waitForDiagnostics(
  uri: vscode.Uri,
  timeout = 15_000,
): Promise<vscode.Diagnostic[]> {
  // If diagnostics are already present, return them
  const existing = vscode.languages.getDiagnostics(uri);
  if (existing.length > 0) {
    return existing;
  }

  return new Promise((resolve, _reject) => {
    const timer = setTimeout(() => {
      disposable.dispose();
      // Return whatever diagnostics exist (may be empty if server is slow)
      resolve(vscode.languages.getDiagnostics(uri));
    }, timeout);

    const disposable = vscode.languages.onDidChangeDiagnostics((e) => {
      const changed = e.uris.find((u) => u.toString() === uri.toString());
      if (changed) {
        clearTimeout(timer);
        disposable.dispose();
        resolve(vscode.languages.getDiagnostics(uri));
      }
    });
  });
}

export async function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
