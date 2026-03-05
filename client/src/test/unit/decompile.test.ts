import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import {
  DEFAULT_EXT_MAP,
  generateDecompilePrc,
  generateBatchDecompilePrc,
  findFilesRecursive,
} from "../../decompile";

suite("DEFAULT_EXT_MAP", () => {
  test("maps .br to .brs", () => {
    assert.strictEqual(DEFAULT_EXT_MAP[".br"], ".brs");
  });

  test("maps .bro to .brs", () => {
    assert.strictEqual(DEFAULT_EXT_MAP[".bro"], ".brs");
  });

  test("maps .wb to .wbs", () => {
    assert.strictEqual(DEFAULT_EXT_MAP[".wb"], ".wbs");
  });

  test("maps .wbo to .wbs", () => {
    assert.strictEqual(DEFAULT_EXT_MAP[".wbo"], ".wbs");
  });

  test("has exactly 4 entries", () => {
    assert.strictEqual(Object.keys(DEFAULT_EXT_MAP).length, 4);
  });
});

suite("generateDecompilePrc", () => {
  test("basic structure", () => {
    const prc = generateDecompilePrc("C:\\test.br", "C:\\tmp\\out.brs", "");
    const lines = prc.split("\n").filter((l) => l.length > 0);
    assert.strictEqual(lines[0], "proc noecho");
    assert.ok(lines.some((l) => l.startsWith("load ")));
    assert.ok(lines.some((l) => l.startsWith("list >")));
    assert.ok(lines.includes("system"));
  });

  test("includes load with absolute path", () => {
    const prc = generateDecompilePrc("C:\\projects\\test.br", "C:\\tmp\\out.brs", "");
    assert.ok(prc.includes('load ":C:\\projects\\test.br"'));
  });

  test("includes list with output path", () => {
    const prc = generateDecompilePrc("C:\\test.br", "C:\\tmp\\decompile0.brs", "");
    assert.ok(prc.includes('list >":C:\\tmp\\decompile0.brs"'));
  });

  test("includes style command when provided", () => {
    const style = "indent 2 45 keywords lower labels mixed comments mixed";
    const prc = generateDecompilePrc("C:\\test.br", "C:\\tmp\\out.brs", style);
    assert.ok(prc.includes(`style ${style}`));
  });

  test("omits style when empty", () => {
    const prc = generateDecompilePrc("C:\\test.br", "C:\\tmp\\out.brs", "");
    assert.ok(!prc.includes("style"));
  });

  test("order is load, style, list, system", () => {
    const prc = generateDecompilePrc("C:\\test.br", "C:\\tmp\\out.brs", "indent 2");
    const lines = prc.split("\n").filter((l) => l.length > 0);
    const loadIdx = lines.findIndex((l) => l.startsWith("load "));
    const styleIdx = lines.findIndex((l) => l.startsWith("style "));
    const listIdx = lines.findIndex((l) => l.startsWith("list >"));
    const sysIdx = lines.indexOf("system");
    assert.ok(loadIdx < styleIdx, "load before style");
    assert.ok(styleIdx < listIdx, "style before list");
    assert.ok(listIdx < sysIdx, "list before system");
  });
});

suite("generateBatchDecompilePrc", () => {
  test("handles multiple files", () => {
    const files = [
      { compiledPath: "C:\\a.br", tmpOutputPath: "C:\\tmp\\a.brs" },
      { compiledPath: "C:\\b.wb", tmpOutputPath: "C:\\tmp\\b.wbs" },
    ];
    const prc = generateBatchDecompilePrc(files, "");
    assert.ok(prc.includes('load ":C:\\a.br"'));
    assert.ok(prc.includes('load ":C:\\b.wb"'));
    assert.ok(prc.includes('list >":C:\\tmp\\a.brs"'));
    assert.ok(prc.includes('list >":C:\\tmp\\b.wbs"'));
  });

  test("applies style to each file", () => {
    const files = [
      { compiledPath: "C:\\a.br", tmpOutputPath: "C:\\tmp\\a.brs" },
      { compiledPath: "C:\\b.br", tmpOutputPath: "C:\\tmp\\b.brs" },
    ];
    const prc = generateBatchDecompilePrc(files, "indent 2");
    const styleCount = (prc.match(/style indent 2/g) || []).length;
    assert.strictEqual(styleCount, 2);
  });

  test("no style when empty", () => {
    const files = [
      { compiledPath: "C:\\a.br", tmpOutputPath: "C:\\tmp\\a.brs" },
    ];
    const prc = generateBatchDecompilePrc(files, "");
    assert.ok(!prc.includes("style"));
  });

  test("starts with proc noecho and ends with system", () => {
    const files = [
      { compiledPath: "C:\\a.br", tmpOutputPath: "C:\\tmp\\a.brs" },
    ];
    const prc = generateBatchDecompilePrc(files, "");
    const lines = prc.split("\n").filter((l) => l.length > 0);
    assert.strictEqual(lines[0], "proc noecho");
    assert.strictEqual(lines[lines.length - 1], "system");
  });

  test("empty files list produces minimal proc", () => {
    const prc = generateBatchDecompilePrc([], "indent 2");
    const lines = prc.split("\n").filter((l) => l.length > 0);
    assert.strictEqual(lines.length, 2);
    assert.strictEqual(lines[0], "proc noecho");
    assert.strictEqual(lines[1], "system");
  });
});

suite("findFilesRecursive", () => {
  let tmpDir: string;

  setup(() => {
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "br-test-"));
  });

  teardown(() => {
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  test("finds files with matching extensions", () => {
    fs.writeFileSync(path.join(tmpDir, "test.br"), "");
    fs.writeFileSync(path.join(tmpDir, "test.wb"), "");
    fs.writeFileSync(path.join(tmpDir, "test.txt"), "");
    const results = findFilesRecursive(tmpDir, [".br", ".wb"]);
    assert.strictEqual(results.length, 2);
    assert.ok(results.some((f) => f.endsWith("test.br")));
    assert.ok(results.some((f) => f.endsWith("test.wb")));
  });

  test("searches subdirectories", () => {
    const subDir = path.join(tmpDir, "sub");
    fs.mkdirSync(subDir);
    fs.writeFileSync(path.join(subDir, "nested.br"), "");
    const results = findFilesRecursive(tmpDir, [".br"]);
    assert.strictEqual(results.length, 1);
    assert.ok(results[0].endsWith("nested.br"));
  });

  test("returns empty for no matches", () => {
    fs.writeFileSync(path.join(tmpDir, "test.txt"), "");
    const results = findFilesRecursive(tmpDir, [".br"]);
    assert.strictEqual(results.length, 0);
  });

  test("returns empty for empty directory", () => {
    const results = findFilesRecursive(tmpDir, [".br"]);
    assert.strictEqual(results.length, 0);
  });

  test("handles nonexistent directory", () => {
    const results = findFilesRecursive(path.join(tmpDir, "nope"), [".br"]);
    assert.strictEqual(results.length, 0);
  });

  test("finds .bro and .wbo extensions", () => {
    fs.writeFileSync(path.join(tmpDir, "test.bro"), "");
    fs.writeFileSync(path.join(tmpDir, "test.wbo"), "");
    const results = findFilesRecursive(tmpDir, [".bro", ".wbo"]);
    assert.strictEqual(results.length, 2);
  });

  test("deeply nested files", () => {
    const deep = path.join(tmpDir, "a", "b", "c");
    fs.mkdirSync(deep, { recursive: true });
    fs.writeFileSync(path.join(deep, "deep.br"), "");
    const results = findFilesRecursive(tmpDir, [".br"]);
    assert.strictEqual(results.length, 1);
    assert.ok(results[0].includes(path.join("a", "b", "c", "deep.br")));
  });
});
