import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument } from "./helper";

suite("Find All References (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("finds all references to a function", async () => {
    const doc = await openDocument("functions.brs");

    // Position on "fnAdd" in the DEF line (line 5: 01000 def fnAdd...)
    const position = new vscode.Position(5, 14);

    const locations = await vscode.commands.executeCommand<vscode.Location[]>(
      "vscode.executeReferenceProvider",
      doc.uri,
      position,
    );

    // fnAdd is defined once (line 5) and called twice (lines 0 and 2)
    assert.ok(locations && locations.length >= 2, `Expected at least 2 references, got ${locations?.length ?? 0}`);
  });

  test("finds all references to a variable", async () => {
    const doc = await openDocument("simple.brs");

    // Position on "X" in line 00110: let X = 42 (col 10)
    const position = new vscode.Position(1, 10);

    const locations = await vscode.commands.executeCommand<vscode.Location[]>(
      "vscode.executeReferenceProvider",
      doc.uri,
      position,
    );

    assert.ok(locations && locations.length >= 1, "Should find at least one reference");
  });
});
