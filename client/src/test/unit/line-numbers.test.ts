import * as assert from "assert";
import {
  extractLineNumber,
  findPreviousLineNumber,
  detectIncrement,
  selectIncrement,
  calculateNextLineNumber,
} from "../../line-numbers";

// Minimal mock that satisfies the TextDocument interface used by line-numbers.ts
function mockDoc(content: string) {
  const lines = content.split("\n");
  return {
    lineCount: lines.length,
    lineAt: (i: number) => ({ text: lines[i] ?? "" }),
  } as any;
}

suite("extractLineNumber", () => {
  test("extracts value and padding from numbered line", () => {
    const result = extractLineNumber("00100 print");
    assert.deepStrictEqual(result, { value: 100, padding: 5 });
  });

  test("extracts 3-digit line number", () => {
    const result = extractLineNumber("100 print");
    assert.deepStrictEqual(result, { value: 100, padding: 3 });
  });

  test("returns null for non-numbered line", () => {
    assert.strictEqual(extractLineNumber("print hello"), null);
  });

  test("returns null for empty line", () => {
    assert.strictEqual(extractLineNumber(""), null);
  });

  test("returns null for comment line without number", () => {
    assert.strictEqual(extractLineNumber("! this is a comment"), null);
  });
});

suite("findPreviousLineNumber", () => {
  test("finds line number on the given line", () => {
    const doc = mockDoc("00100 print\n00110 stop");
    const result = findPreviousLineNumber(doc, 1);
    assert.deepStrictEqual(result, { info: { value: 110, padding: 5 }, line: 1 });
  });

  test("walks backward past blank lines", () => {
    const doc = mockDoc("00100 print\n\n\n00120 stop");
    const result = findPreviousLineNumber(doc, 2);
    assert.deepStrictEqual(result, { info: { value: 100, padding: 5 }, line: 0 });
  });

  test("walks backward past comment lines", () => {
    const doc = mockDoc("00100 print\n! comment\n00120 stop");
    const result = findPreviousLineNumber(doc, 1);
    assert.deepStrictEqual(result, { info: { value: 100, padding: 5 }, line: 0 });
  });

  test("returns null when no numbered line exists", () => {
    const doc = mockDoc("! comment\nprint hello");
    assert.strictEqual(findPreviousLineNumber(doc, 1), null);
  });
});

suite("detectIncrement", () => {
  test("detects increment of 10", () => {
    const doc = mockDoc("00100 let x = 1\n00110 let y = 2\n");
    assert.strictEqual(detectIncrement(doc, 2, 10), 10);
  });

  test("detects increment of 5", () => {
    const doc = mockDoc("00100 let x = 1\n00105 let y = 2\n");
    assert.strictEqual(detectIncrement(doc, 2, 10), 5);
  });

  test("detects increment of 20", () => {
    const doc = mockDoc("00100 let x = 1\n00120 let y = 2\n");
    assert.strictEqual(detectIncrement(doc, 2, 10), 20);
  });

  test("falls back to default when only one numbered line", () => {
    const doc = mockDoc("00100 let x = 1\n");
    assert.strictEqual(detectIncrement(doc, 1, 10), 10);
  });
});

suite("selectIncrement", () => {
  test("no constraint uses preferred increment", () => {
    assert.strictEqual(selectIncrement(10, null), 10);
  });

  test("returns 0 when no space", () => {
    assert.strictEqual(selectIncrement(10, 0), 0);
  });

  test("fits within maxSpace", () => {
    // maxSpace=7 with detected=10 → can't use 10, fallback to 2
    assert.strictEqual(selectIncrement(10, 7), 2);
  });

  test("increment of 2 when maxSpace is 7 and detected is 2", () => {
    assert.strictEqual(selectIncrement(2, 7), 2);
  });

  test("preferred increment when fits", () => {
    assert.strictEqual(selectIncrement(10, 15), 10);
  });

  test("falls back to lower preferred when constrained", () => {
    // detected=20, maxSpace=5 → falls back to 2
    assert.strictEqual(selectIncrement(20, 5), 2);
  });
});

suite("calculateNextLineNumber", () => {
  test("calculates next line number with increment of 10", () => {
    const doc = mockDoc("00100 let x = 1\n00110 let y = 2\n");
    const result = calculateNextLineNumber(doc, 2, 10, 5);
    assert.strictEqual(result, "00120");
  });

  test("preserves 3-digit format", () => {
    const doc = mockDoc("100 let x = 1\n110 let y = 2\n");
    const result = calculateNextLineNumber(doc, 2, 10, 5);
    assert.strictEqual(result, "120");
  });

  test("returns null after continuation line", () => {
    const doc = mockDoc("00100 print x !:\n");
    const result = calculateNextLineNumber(doc, 1, 10, 5);
    assert.strictEqual(result, null);
  });

  test("returns null when no previous line number", () => {
    const doc = mockDoc("! just a comment\n");
    const result = calculateNextLineNumber(doc, 1, 10, 5);
    assert.strictEqual(result, null);
  });

  test("fits between existing lines", () => {
    // After 00100, before 00120 → 00110
    const doc = mockDoc("00100 let x = 1\n00120 let y = 2");
    const result = calculateNextLineNumber(doc, 1, 10, 5);
    assert.strictEqual(result, "00110");
  });

  test("smaller increment when constrained", () => {
    // After 00100, before 00105, increment=10 won't fit → 00102
    const doc = mockDoc("00100 let x = 1\n00105 let y = 2");
    const result = calculateNextLineNumber(doc, 1, 10, 5);
    assert.strictEqual(result, "00102");
  });

  test("returns null when no space between lines", () => {
    const doc = mockDoc("00100 let x = 1\n00101 let y = 2");
    const result = calculateNextLineNumber(doc, 1, 10, 5);
    assert.strictEqual(result, null);
  });

  test("normal increment at end of file", () => {
    const doc = mockDoc("00100 let x = 1\n00110 let y = 2");
    // Insert after last line (line 2 = past end)
    const result = calculateNextLineNumber(doc, 2, 10, 5);
    assert.strictEqual(result, "00120");
  });

  test("returns null at start of file", () => {
    const doc = mockDoc("00100 let x = 1");
    const result = calculateNextLineNumber(doc, 0, 10, 5);
    assert.strictEqual(result, null);
  });
});
