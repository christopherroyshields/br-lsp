import * as assert from "assert";
import { parseBrOutput, parseBrState, stripAnsi, generatePrc, PrcPaths } from "../../compile";

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

suite("generatePrc", () => {
  test("numbered source uses lexionly.brs", () => {
    const prc = generatePrc({
      sourceBase: "1_test.brs",
      tempFile: "temp1",
      outputBase: "1_test",
      outputExt: ".br",
      hasNumbers: true,
    });
    assert.ok(prc.includes("subproc lexionly.brs"));
    assert.ok(!prc.includes("linenum.brs"));
  });

  test("unnumbered source uses linenum.brs", () => {
    const prc = generatePrc({
      sourceBase: "2_test.brs",
      tempFile: "temp2",
      outputBase: "2_test",
      outputExt: ".br",
      hasNumbers: false,
    });
    assert.ok(prc.includes("subproc linenum.brs"));
    assert.ok(!prc.includes("lexionly.brs"));
  });

  test("uses backslash paths regardless of platform", () => {
    const prc = generatePrc({
      sourceBase: "3_test.brs",
      tempFile: "temp3",
      outputBase: "3_test",
      outputExt: ".br",
      hasNumbers: true,
    });
    assert.ok(prc.includes('Infile$="tmp\\3_test.brs"'));
    assert.ok(prc.includes('Outfile$="tmp\\temp3"'));
    assert.ok(prc.includes("subproc tmp\\temp3"));
    // No forward slashes in generated paths
    assert.ok(!prc.includes("tmp/"));
  });

  test("variable assignments on lines 00025/00026", () => {
    const prc = generatePrc({
      sourceBase: "src.brs",
      tempFile: "tmp1",
      outputBase: "src",
      outputExt: ".br",
      hasNumbers: false,
    });
    assert.ok(prc.includes("00025 Infile$="));
    assert.ok(prc.includes("00026 Outfile$="));
  });

  test("contains proc structure", () => {
    const prc = generatePrc({
      sourceBase: "x.brs",
      tempFile: "t1",
      outputBase: "x",
      outputExt: ".br",
      hasNumbers: true,
    });
    const lines = prc.split("\n").filter((l) => l.length > 0);
    assert.strictEqual(lines[0], "proc noecho");
    assert.ok(lines.some((l) => l.startsWith("subproc ")));
    assert.ok(lines.includes("run"));
    assert.ok(lines.includes("clear"));
    assert.ok(lines.includes("system"));
  });

  test("save/replace logic with exists check", () => {
    const prc = generatePrc({
      sourceBase: "f.brs",
      tempFile: "tf",
      outputBase: "f",
      outputExt: ".br",
      hasNumbers: false,
    });
    assert.ok(prc.includes('skip PROGRAM_REPLACE if exists("tmp\\f")'));
    assert.ok(prc.includes('skip PROGRAM_REPLACE if exists("tmp\\f.br")'));
    assert.ok(prc.includes('save "tmp\\f.br"'));
    assert.ok(prc.includes(":PROGRAM_REPLACE"));
    assert.ok(prc.includes('replace "tmp\\f.br"'));
  });

  test("handles .wbs/.wb extension", () => {
    const prc = generatePrc({
      sourceBase: "page.wbs",
      tempFile: "temp4",
      outputBase: "page",
      outputExt: ".wb",
      hasNumbers: true,
    });
    assert.ok(prc.includes('Infile$="tmp\\page.wbs"'));
    assert.ok(prc.includes('save "tmp\\page.wb"'));
    assert.ok(prc.includes('replace "tmp\\page.wb"'));
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
