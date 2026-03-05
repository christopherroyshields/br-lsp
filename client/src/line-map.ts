import * as fs from "fs";
import * as path from "path";

export interface LineMap {
  /** Map from BR line number → editor line (1-based) */
  brToEditor: Map<number, number>;
  /** Map from editor line (1-based) → BR line number */
  editorToBr: Map<number, number>;
}

export function buildLineMap(sourcePath: string): LineMap {
  const brToEditor = new Map<number, number>();
  const editorToBr = new Map<number, number>();

  // Check for a .map sourcemap file alongside the source
  const parsed = path.parse(sourcePath);
  const mapPath = path.join(parsed.dir, parsed.name + ".map");
  try {
    const mapContent = fs.readFileSync(mapPath, "utf-8");
    for (const line of mapContent.split(/\r?\n/)) {
      const trimmed = line.trim();
      if (!trimmed) continue;
      const parts = trimmed.split(",");
      if (parts.length >= 2) {
        const brLine = parseInt(parts[0], 10);
        const sourceLine = parseInt(parts[1], 10);
        if (!isNaN(brLine) && !isNaN(sourceLine)) {
          brToEditor.set(brLine, sourceLine);
          editorToBr.set(sourceLine, brLine);
        }
      }
    }
    return { brToEditor, editorToBr };
  } catch {
    // No .map file — fall through to regex parsing
  }

  let content: string;
  try {
    content = fs.readFileSync(sourcePath, "latin1");
  } catch {
    return { brToEditor, editorToBr };
  }

  const lines = content.split(/\r?\n/);
  const lineNumRe = /^\s*(\d{3,5})\s/;

  for (let i = 0; i < lines.length; i++) {
    const match = lineNumRe.exec(lines[i]);
    if (match) {
      const brLine = parseInt(match[1], 10);
      const editorLine = i + 1; // 1-based
      brToEditor.set(brLine, editorLine);
      editorToBr.set(editorLine, brLine);
    }
  }

  return { brToEditor, editorToBr };
}
