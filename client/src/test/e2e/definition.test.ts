import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument } from "./helper";

suite("Go to Definition (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("function call resolves to DEF line", async () => {
    const doc = await openDocument("functions.brs");

    // Position on "fnAdd" in line 00100: let X = fnAdd(1, 2)
    const position = new vscode.Position(0, 18);

    const locations = await vscode.commands.executeCommand<vscode.Location[]>(
      "vscode.executeDefinitionProvider",
      doc.uri,
      position,
    );

    assert.ok(locations && locations.length > 0, "Should find at least one definition");
    // DEF fnAdd is on line index 5 (01000 def fnAdd...)
    assert.strictEqual(locations[0].range.start.line, 5);
  });

  test("go to definition on line number reference", async () => {
    const doc = await openDocument("simple.brs");

    // Position on "100" in line 00130: goto 100
    const position = new vscode.Position(3, 14);

    const locations = await vscode.commands.executeCommand<vscode.Location[]>(
      "vscode.executeDefinitionProvider",
      doc.uri,
      position,
    );

    assert.ok(locations && locations.length > 0, "Should find definition for line number");
    assert.strictEqual(locations[0].range.start.line, 0);
  });
});
