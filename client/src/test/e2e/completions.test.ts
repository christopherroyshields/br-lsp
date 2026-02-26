import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument } from "./helper";

suite("Completions (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("completion list includes keywords", async () => {
    const doc = await openDocument("simple.brs");

    // Trigger completion at end of a line
    const position = new vscode.Position(0, 10);
    const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      doc.uri,
      position,
    );

    assert.ok(completions, "Should return completion list");
    const labels = completions.items.map((item) =>
      typeof item.label === "string" ? item.label : item.label.label,
    );
    // Check for at least one BR keyword
    const hasKeyword = labels.some((l) => /^(print|let|goto|if|dim|open|read|write|close)$/i.test(l));
    assert.ok(hasKeyword, `Expected at least one BR keyword in completions, got: ${labels.slice(0, 10).join(", ")}`);
  });

  test("completion list includes user-defined functions", async () => {
    const doc = await openDocument("functions.brs");

    // Trigger completion somewhere in the file
    const position = new vscode.Position(0, 18);
    const completions = await vscode.commands.executeCommand<vscode.CompletionList>(
      "vscode.executeCompletionItemProvider",
      doc.uri,
      position,
    );

    assert.ok(completions, "Should return completion list");
    const labels = completions.items.map((item) =>
      typeof item.label === "string" ? item.label : item.label.label,
    );
    const hasFnAdd = labels.some((l) => /fnadd/i.test(l));
    assert.ok(hasFnAdd, `Expected fnAdd in completions, got: ${labels.slice(0, 20).join(", ")}`);
  });
});
