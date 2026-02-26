import * as assert from "assert";
import * as vscode from "vscode";
import { activate, openDocument, waitForDiagnostics } from "./helper";

suite("Diagnostics (E2E)", () => {
  suiteSetup(async () => {
    await activate();
  });

  test("missing FNEND produces diagnostic", async () => {
    const doc = await openDocument("diagnostics.brs");
    const diagnostics = await waitForDiagnostics(doc.uri);

    const missingFnend = diagnostics.filter((d) => /fnend/i.test(d.message));
    assert.ok(missingFnend.length > 0, "Should report missing FNEND diagnostic");
  });

  test("duplicate function produces diagnostic", async () => {
    const doc = await openDocument("diagnostics.brs");
    const diagnostics = await waitForDiagnostics(doc.uri);

    const dupes = diagnostics.filter((d) => /already defined/i.test(d.message));
    assert.ok(dupes.length > 0, "Should report duplicate function diagnostic (message contains 'already defined')");
  });

  test("undefined function produces warning", async () => {
    const doc = await openDocument("diagnostics.brs");
    const diagnostics = await waitForDiagnostics(doc.uri);

    const undefined_ = diagnostics.filter((d) => /undefined|not defined/i.test(d.message));
    assert.ok(undefined_.length > 0, "Should report undefined function warning");
  });

  test("clean file produces no error diagnostics", async () => {
    const doc = await openDocument("functions.brs");
    // openDocument already waits for the server to process the file,
    // so diagnostics are available immediately.
    const diagnostics = vscode.languages.getDiagnostics(doc.uri);

    const errors = diagnostics.filter((d) => d.severity === vscode.DiagnosticSeverity.Error);
    assert.strictEqual(errors.length, 0, `Expected no error diagnostics, got: ${errors.map((d) => d.message).join(", ")}`);
  });
});
