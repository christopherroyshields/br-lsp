import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument } from "./helper";

suite("Rename (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("rename function updates definition and calls", async () => {
    const doc = await openDocument("functions.brs");

    // Position on "fnAdd" in the DEF line (line 5: 01000 def fnAdd...)
    const position = new vscode.Position(5, 14);

    const edit = await vscode.commands.executeCommand<vscode.WorkspaceEdit>(
      "vscode.executeDocumentRenameProvider",
      doc.uri,
      position,
      "fnSum",
    );

    assert.ok(edit, "Should return a workspace edit");
    const entries = edit.entries();
    assert.ok(entries.length > 0, "Workspace edit should have entries");

    // Count total text edits across all files
    let totalEdits = 0;
    for (const [, edits] of entries) {
      totalEdits += edits.length;
    }
    // fnAdd appears 3 times: def + 2 calls
    assert.ok(totalEdits >= 3, `Expected at least 3 edits, got ${totalEdits}`);
  });

  test("rename variable updates all occurrences", async () => {
    const doc = await openDocument("simple.brs");

    // Position on "X" in line 00110: let X = 42 (col 10)
    const position = new vscode.Position(1, 10);

    const edit = await vscode.commands.executeCommand<vscode.WorkspaceEdit>(
      "vscode.executeDocumentRenameProvider",
      doc.uri,
      position,
      "NewVar",
    );

    assert.ok(edit, "Should return a workspace edit");
    const entries = edit.entries();
    assert.ok(entries.length > 0, "Workspace edit should have entries");

    // Count total text edits across all files
    let totalEdits = 0;
    for (const [, edits] of entries) {
      totalEdits += edits.length;
    }
    // X appears 3 times: line 110 (let X), line 140 (let X = X + 1)
    assert.ok(totalEdits >= 3, `Expected at least 3 edits, got ${totalEdits}`);
  });
});
