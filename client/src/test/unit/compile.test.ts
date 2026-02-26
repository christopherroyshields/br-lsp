import * as assert from "assert";
import { parseBrOutput, parseBrState, stripAnsi } from "../../compile";

suite("parseBrOutput", () => {
  test("full error format with clause", () => {
    const result = parseBrOutput("414800430:01ERROR  C:\\lexi.");
    assert.strictEqual(result, "Error 4148 at line 430:1 in C:\\lexi");
  });

  test("full error format without clause", () => {
    const result = parseBrOutput("414800430:00ERROR  C:\\test.");
    assert.strictEqual(result, "Error 4148 at line 430 in C:\\test");
  });

  test("full error format without program name", () => {
    const result = parseBrOutput("414800430:00ERROR");
    assert.strictEqual(result, "Error 4148 at line 430");
  });

  test("short error format - 4 digits", () => {
    const result = parseBrOutput("1026ERROR");
    assert.strictEqual(result, "Error 1026");
  });

  test("short error format - 3 digits", () => {
    const result = parseBrOutput("100ERROR");
    assert.strictEqual(result, "Error 100");
  });

  test("fatal error", () => {
    const result = parseBrOutput("Fatal error called with message: Out of memory");
    assert.strictEqual(result, "Out of memory");
  });

  test("no error returns null", () => {
    const result = parseBrOutput("READY");
    assert.strictEqual(result, null);
  });

  test("clause 0 is suppressed", () => {
    const result = parseBrOutput("020000100:00ERROR  test.");
    assert.strictEqual(result, "Error 200 at line 100 in test");
  });

  test("program name trailing dot is trimmed", () => {
    const result = parseBrOutput("414800430:01ERROR  myprogram.");
    assert.strictEqual(result, "Error 4148 at line 430:1 in myprogram");
  });

  test("empty string returns null", () => {
    const result = parseBrOutput("");
    assert.strictEqual(result, null);
  });
});

suite("parseBrState", () => {
  test("READY", () => {
    assert.strictEqual(parseBrState("READY"), "READY");
  });

  test("PROC", () => {
    assert.strictEqual(parseBrState("PROC"), "PROC");
  });

  test("RUN", () => {
    assert.strictEqual(parseBrState("RUN"), "RUN");
  });

  test("INPUT", () => {
    assert.strictEqual(parseBrState("INPUT"), "INPUT");
  });

  test("KEYIN", () => {
    assert.strictEqual(parseBrState("KEYIN"), "KEYIN");
  });

  test("PRINT", () => {
    assert.strictEqual(parseBrState("PRINT"), "PRINT");
  });

  test("STOP", () => {
    assert.strictEqual(parseBrState("STOP"), "STOP");
  });

  test("PAUSE", () => {
    assert.strictEqual(parseBrState("PAUSE"), "PAUSE");
  });

  test("DEBUG", () => {
    assert.strictEqual(parseBrState("DEBUG"), "DEBUG");
  });

  test("SYNTAX", () => {
    assert.strictEqual(parseBrState("SYNTAX"), "SYNTAX");
  });

  test("case insensitive", () => {
    assert.strictEqual(parseBrState("ready"), "READY");
    assert.strictEqual(parseBrState("Ready"), "READY");
  });

  test("non-matching returns null", () => {
    assert.strictEqual(parseBrState("UNKNOWN"), null);
    assert.strictEqual(parseBrState(""), null);
    assert.strictEqual(parseBrState("some random text"), null);
  });
});

suite("stripAnsi", () => {
  test("strips CSI escape sequences", () => {
    assert.strictEqual(stripAnsi("\x1b[31mred\x1b[0m"), "red");
  });

  test("preserves normal text", () => {
    assert.strictEqual(stripAnsi("hello world"), "hello world");
  });

  test("handles multiple sequences", () => {
    assert.strictEqual(stripAnsi("\x1b[1m\x1b[31mbold red\x1b[0m normal"), "bold red normal");
  });

  test("strips character set sequences", () => {
    assert.strictEqual(stripAnsi("\x1b(Btext\x1b(0"), "text");
  });

  test("strips private mode sequences", () => {
    assert.strictEqual(stripAnsi("\x1b[?25htext\x1b[?25l"), "text");
  });

  test("empty string returns empty", () => {
    assert.strictEqual(stripAnsi(""), "");
  });
});
