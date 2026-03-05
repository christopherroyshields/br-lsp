import * as assert from "assert";
import * as fs from "fs";
import * as path from "path";
import * as os from "os";
import { buildLineMap } from "../../line-map";

let tmpDir: string;

setup(() => {
  tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "br-debug-test-"));
});

teardown(() => {
  fs.rmSync(tmpDir, { recursive: true, force: true });
});

suite("buildLineMap", () => {
  test("reads sourcemap CSV file when present", () => {
    const source = path.join(tmpDir, "prog.brs");
    const mapFile = path.join(tmpDir, "prog.map");
    fs.writeFileSync(source, "print 'hello'\nprint 'world'\n");
    fs.writeFileSync(mapFile, "100,1\n110,2\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.get(100), 1);
    assert.strictEqual(map.brToEditor.get(110), 2);
    assert.strictEqual(map.editorToBr.get(1), 100);
    assert.strictEqual(map.editorToBr.get(2), 110);
  });

  test("sourcemap works for .br files too", () => {
    const source = path.join(tmpDir, "prog.br");
    const mapFile = path.join(tmpDir, "prog.map");
    fs.writeFileSync(source, "");
    fs.writeFileSync(mapFile, "200,5\n210,10\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.get(200), 5);
    assert.strictEqual(map.brToEditor.get(210), 10);
  });

  test("sourcemap ignores blank lines", () => {
    const source = path.join(tmpDir, "prog.brs");
    const mapFile = path.join(tmpDir, "prog.map");
    fs.writeFileSync(source, "");
    fs.writeFileSync(mapFile, "100,1\n\n110,2\n\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.size, 2);
    assert.strictEqual(map.editorToBr.size, 2);
  });

  test("falls back to regex when no .map file exists", () => {
    const source = path.join(tmpDir, "prog.brs");
    fs.writeFileSync(source, "00100 print 'hello'\n00110 print 'world'\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.get(100), 1);
    assert.strictEqual(map.brToEditor.get(110), 2);
    assert.strictEqual(map.editorToBr.get(1), 100);
    assert.strictEqual(map.editorToBr.get(2), 110);
  });

  test("regex fallback skips lines without numbers", () => {
    const source = path.join(tmpDir, "prog.brs");
    fs.writeFileSync(source, "! comment\n00100 print 'hello'\n\n00110 stop\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.size, 2);
    assert.strictEqual(map.brToEditor.get(100), 2);
    assert.strictEqual(map.brToEditor.get(110), 4);
  });

  test("sourcemap takes priority over regex", () => {
    // Source has line numbers, but .map file maps them differently
    const source = path.join(tmpDir, "prog.brs");
    const mapFile = path.join(tmpDir, "prog.map");
    fs.writeFileSync(source, "00100 print 'hello'\n00110 print 'world'\n");
    fs.writeFileSync(mapFile, "100,5\n110,10\n");

    const map = buildLineMap(source);
    // Should use .map values, not regex-parsed values
    assert.strictEqual(map.brToEditor.get(100), 5);
    assert.strictEqual(map.brToEditor.get(110), 10);
  });

  test("returns empty maps for nonexistent file", () => {
    const map = buildLineMap(path.join(tmpDir, "nofile.brs"));
    assert.strictEqual(map.brToEditor.size, 0);
    assert.strictEqual(map.editorToBr.size, 0);
  });

  test("returns empty maps for file with no line numbers", () => {
    const source = path.join(tmpDir, "prog.brs");
    fs.writeFileSync(source, "print 'hello'\nprint 'world'\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.size, 0);
    assert.strictEqual(map.editorToBr.size, 0);
  });

  test("handles CRLF in sourcemap", () => {
    const source = path.join(tmpDir, "prog.brs");
    const mapFile = path.join(tmpDir, "prog.map");
    fs.writeFileSync(source, "");
    fs.writeFileSync(mapFile, "100,1\r\n110,2\r\n");

    const map = buildLineMap(source);
    assert.strictEqual(map.brToEditor.get(100), 1);
    assert.strictEqual(map.brToEditor.get(110), 2);
  });
});
