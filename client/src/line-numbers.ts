import * as vscode from "vscode";

interface LineNumberInfo {
  value: number;
  padding: number;
}

const LINE_NUMBER_RE = /^(\d+)\s/;
const CONTINUATION_RE = /!:\s*$/;
const PREFERRED_INCREMENTS = [1, 2, 10, 20, 100];

function extractLineNumber(lineText: string): LineNumberInfo | null {
  const match = lineText.match(LINE_NUMBER_RE);
  if (!match) {
    return null;
  }
  const text = match[1];
  const value = parseInt(text, 10);
  if (isNaN(value)) {
    return null;
  }
  return { value, padding: text.length };
}

function findPreviousLineNumber(
  document: vscode.TextDocument,
  startLine: number,
): { info: LineNumberInfo; line: number } | null {
  for (let i = startLine; i >= 0; i--) {
    const info = extractLineNumber(document.lineAt(i).text);
    if (info) {
      return { info, line: i };
    }
  }
  return null;
}

function detectIncrement(document: vscode.TextDocument, currentLine: number, defaultIncrement: number): number {
  const lineNumbers: number[] = [];
  for (let i = currentLine - 1; i >= Math.max(0, currentLine - 10); i--) {
    const info = extractLineNumber(document.lineAt(i).text);
    if (info) {
      lineNumbers.push(info.value);
    }
    if (lineNumbers.length >= 2) {
      break;
    }
  }
  if (lineNumbers.length >= 2) {
    const increment = lineNumbers[0] - lineNumbers[1];
    if (increment > 0) {
      return increment;
    }
  }
  return defaultIncrement;
}

function selectIncrement(detected: number, maxSpace: number | null): number {
  if (maxSpace !== null) {
    if (maxSpace < 1) {
      return 0;
    }
    if (detected > maxSpace) {
      let best = 1;
      for (const inc of PREFERRED_INCREMENTS) {
        if (inc <= maxSpace) {
          best = inc;
        } else {
          break;
        }
      }
      return best;
    }
    let best = 1;
    for (const inc of PREFERRED_INCREMENTS) {
      if (inc <= detected && inc <= maxSpace) {
        best = inc;
      } else if (inc > detected) {
        break;
      }
    }
    return best;
  }

  let best = detected;
  for (const inc of PREFERRED_INCREMENTS) {
    if (inc <= detected) {
      best = inc;
    } else {
      break;
    }
  }
  return best;
}

function calculateNextLineNumber(
  document: vscode.TextDocument,
  currentLine: number,
  configIncrement: number,
  configPadding: number,
): string | null {
  const prevSearchStart = currentLine - 1;
  if (prevSearchStart < 0) {
    return null;
  }

  // Don't add line number after continuation
  if (CONTINUATION_RE.test(document.lineAt(prevSearchStart).text)) {
    return null;
  }

  const prev = findPreviousLineNumber(document, prevSearchStart);
  if (!prev) {
    return null;
  }

  // Check if there's a numbered line at the insertion point constraining us
  let maxSpace: number | null = null;
  if (currentLine < document.lineCount) {
    const nextInfo = extractLineNumber(document.lineAt(currentLine).text);
    if (nextInfo) {
      maxSpace = nextInfo.value - prev.info.value - 1;
      if (maxSpace < 1) {
        return null;
      }
    }
  }

  const detected = detectIncrement(document, currentLine, configIncrement);
  const increment = selectIncrement(detected, maxSpace);
  if (increment === 0) {
    return null;
  }

  const nextValue = prev.info.value + increment;
  const padding = prev.info.padding || configPadding;
  const maxValue = Math.pow(10, padding) - 1;
  if (nextValue > maxValue) {
    return null;
  }

  return nextValue.toString().padStart(padding, "0");
}

function getContinuationIndent(lineText: string): string {
  const match = lineText.match(/^(\d+)(\s+)/);
  return match?.[2] ?? " ";
}

export function activateLineNumbers(context: vscode.ExtensionContext) {
  let hasShownOverflowWarning = false;

  context.subscriptions.push(
    vscode.commands.registerTextEditorCommand("br-lsp.autoInsertLineNumber", (editor, edit) => {
      const config = vscode.workspace.getConfiguration("br-lsp");
      const enabled = config.get<boolean>("autoLineNumbers.enabled", true);

      const position = editor.selection.active;

      if (!enabled) {
        edit.insert(position, "\n");
        return;
      }

      const configIncrement = config.get<number>("autoLineNumbers.increment", 10);
      const configPadding = config.get<number>("autoLineNumbers.zeroPadding", 5);
      const currentLineIndex = position.line;
      const currentLineText = editor.document.lineAt(currentLineIndex).text;

      // Only auto-insert if current line has a line number
      const currentLineInfo = extractLineNumber(currentLineText);
      if (!currentLineInfo) {
        edit.insert(position, "\n");
        return;
      }

      // Continuation line: indent instead of numbering
      if (CONTINUATION_RE.test(currentLineText)) {
        const indent = getContinuationIndent(currentLineText);
        edit.insert(position, "\n" + indent);
        return;
      }

      const nextLineNumber = calculateNextLineNumber(
        editor.document,
        currentLineIndex + 1,
        configIncrement,
        configPadding,
      );

      if (nextLineNumber === null) {
        const maxValue = Math.pow(10, configPadding) - 1;
        if (currentLineInfo.value + configIncrement > maxValue && !hasShownOverflowWarning) {
          vscode.window.showWarningMessage(
            `BR: Line number overflow detected (exceeds ${maxValue}). Auto line numbering disabled.`,
          );
          hasShownOverflowWarning = true;
        }
        edit.insert(position, "\n");
        return;
      }

      if (!editor.selection.isEmpty) {
        edit.delete(editor.selection);
      }

      edit.insert(position, "\n" + nextLineNumber + " ");
    }),
  );
}
