import * as assert from "assert";
import * as vscode from "vscode";
import * as path from "path";
import { resolveVariables } from "../../run";

// Minimal mock for vscode.ExtensionContext — only extensionPath is used
function mockContext(extensionPath: string): any {
  return { extensionPath };
}

suite("resolveVariables", () => {
  test("substitutes ${extensionPath}", () => {
    const ctx = mockContext("/home/user/.vscode/extensions/br-lsp");
    const result = resolveVariables("${extensionPath}/Lexi/brlinux", undefined, ctx);
    assert.ok(result.includes("Lexi"));
    assert.ok(result.includes("brlinux"));
    assert.ok(!result.includes("${extensionPath}"));
  });

  test("substitutes ${file}", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "myapp", "main.brs");
    const result = resolveVariables("${file}", activeFile, ctx);
    assert.strictEqual(result, path.normalize(activeFile));
  });

  test("substitutes ${fileDirname}", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "myapp", "main.brs");
    const result = resolveVariables("${fileDirname}", activeFile, ctx);
    assert.strictEqual(result, path.normalize(path.join("/projects", "myapp")));
  });

  test("substitutes ${fileBasename}", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "myapp", "main.brs");
    const result = resolveVariables("${fileBasename}", activeFile, ctx);
    // Result is "main.brs" but resolveVariables joins relative results with
    // the workspace folder when one exists in the test runner environment.
    assert.ok(result.endsWith("main.brs"));
    assert.ok(!result.includes("${fileBasename}"));
  });

  test("substitutes ${fileBasenameNoExtension}", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "myapp", "main.brs");
    const result = resolveVariables("${fileBasenameNoExtension}", activeFile, ctx);
    // Basename "main" is relative, so it gets joined with workspace folder
    assert.ok(result.endsWith("main"));
    assert.ok(!result.includes("${fileBasenameNoExtension}"));
  });

  test("relative result joined with workspace folder", () => {
    const wsFolder = vscode.workspace.workspaceFolders?.[0];
    if (!wsFolder) {
      return; // skip if no workspace
    }
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "myapp", "main.brs");
    const result = resolveVariables("${fileBasename}", activeFile, ctx);
    // "main.brs" is relative → joined with workspace folder
    assert.strictEqual(result, path.normalize(path.join(wsFolder.uri.fsPath, "main.brs")));
  });

  test("file-related vars left unsubstituted when activeFile is undefined", () => {
    const ctx = mockContext("/ext");
    const result = resolveVariables("${extensionPath}/bin/${fileBasename}", undefined, ctx);
    // extensionPath resolved, fileBasename left as-is
    assert.ok(!result.includes("${extensionPath}"));
    assert.ok(result.includes("${fileBasename}"));
  });

  test("multiple variables in one string", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "app", "test.brs");
    const result = resolveVariables(
      "${extensionPath}/Lexi/${fileBasenameNoExtension}",
      activeFile,
      ctx,
    );
    assert.ok(!result.includes("${extensionPath}"));
    assert.ok(!result.includes("${fileBasenameNoExtension}"));
    assert.ok(result.includes("Lexi"));
    assert.ok(result.includes("test"));
  });

  test("result is normalized", () => {
    const ctx = mockContext("/ext");
    const activeFile = path.join("/projects", "app", "src.brs");
    const result = resolveVariables("${extensionPath}//extra///slashes", activeFile, ctx);
    assert.strictEqual(result, path.normalize("/ext//extra///slashes"));
  });
});
