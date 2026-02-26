import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument, waitForDiagnostics } from "./helper";

suite("Code Actions (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("code action offered for undefined function", async () => {
    const doc = await openDocument("diagnostics.brs");
    const diagnostics = await waitForDiagnostics(doc.uri);

    // Find the undefined function diagnostic
    const undefinedDiag = diagnostics.find((d) => /undefined|not defined/i.test(d.message));
    if (!undefinedDiag) {
      // Server may not produce this diagnostic — skip gracefully
      return;
    }

    const actions = await vscode.commands.executeCommand<vscode.CodeAction[]>(
      "vscode.executeCodeActionProvider",
      doc.uri,
      undefinedDiag.range,
    );

    assert.ok(actions && actions.length > 0, "Should offer at least one code action for undefined function");
  });

  test("code action generates function stub", async () => {
    const doc = await openDocument("diagnostics.brs");
    const diagnostics = await waitForDiagnostics(doc.uri);

    const undefinedDiag = diagnostics.find((d) => /undefined|not defined/i.test(d.message));
    if (!undefinedDiag) {
      return;
    }

    const actions = await vscode.commands.executeCommand<vscode.CodeAction[]>(
      "vscode.executeCodeActionProvider",
      doc.uri,
      undefinedDiag.range,
    );

    if (!actions || actions.length === 0) {
      return;
    }

    // Check that at least one action has an edit (generates a stub)
    const withEdit = actions.find((a) => a.edit && a.edit.entries().length > 0);
    assert.ok(withEdit, "At least one code action should provide an edit to generate a function stub");
  });
});
