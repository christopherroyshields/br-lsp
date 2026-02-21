import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { spawn } from "child_process";

const LOADER = "/lib64/ld-linux-x86-64.so.2";
// Strip ANSI escape sequences and terminal control codes from PTY output
const ANSI_RE = /\x1b\[[0-9;]*[A-Za-z]|\x1b[()][0-9A-Za-z]|\x1b\[\?[0-9;]*[A-Za-z]/g;

let outputChannel: vscode.OutputChannel;
let rawOutputChannel: vscode.OutputChannel;
let autoCompileState: Map<string, boolean> = new Map();
let autoCompileStatusBarItem: vscode.StatusBarItem;
let compiling = false;
let compileId = 0;

export const EXT_MAP: Record<string, string> = {
  ".brs": ".br",
  ".wbs": ".wb",
};

export function getLexiPath(context: vscode.ExtensionContext): string {
  return path.join(context.extensionPath, "Lexi");
}

function ensureExecutable(filePath: string): void {
  try {
    fs.accessSync(filePath, fs.constants.X_OK);
  } catch {
    fs.chmodSync(filePath, 0o755);
  }
}

function hasLineNumbers(filePath: string): boolean {
  const content = fs.readFileSync(filePath, "latin1");
  const firstLine = content.split(/\r?\n/).find((line) => line.trim().length > 0);
  if (!firstLine) {
    return false;
  }
  return /^\s*\d{3,5}\s/.test(firstLine);
}

interface PrcPaths {
  sourceBase: string; // e.g. "3_test2.brs" — source copy in tmp/
  tempFile: string; // e.g. "temp3" — Lexi preprocessor output in tmp/
  outputBase: string; // e.g. "3_test2" — compiled output name (no ext) in tmp/
  outputExt: string; // e.g. ".br"
  hasNumbers: boolean;
}

function generatePrc(p: PrcPaths): string {
  // BR uses backslash paths internally even on Linux.
  // Always run through the Lexi preprocessor (fnApplyLexi from lexi.br) which
  // handles line continuation, backtick strings, #SELECT#/#CASE#, #DEFINE#, etc.
  // - linenum.brs: Lexi + add line numbers (for unnumbered source)
  // - lexionly.brs: Lexi only, DontAddLineNumbers=1 (for already-numbered source)
  // Variable assignments use line 00025/00026 so they run AFTER the dim on line 00020
  // in the subproc'd .brs file (dim resets variables to empty in BR).
  const lexiEntry = p.hasNumbers ? "lexionly.brs" : "linenum.brs";
  let prc = "";
  prc += "proc noecho\n";
  prc += `subproc ${lexiEntry}\n`;
  prc += `00025 Infile$="tmp\\${p.sourceBase}"\n`;
  prc += `00026 Outfile$="tmp\\${p.tempFile}"\n`;
  prc += "run\n";
  prc += "clear\n";
  prc += `subproc tmp\\${p.tempFile}\n`;
  prc += `skip PROGRAM_REPLACE if exists("tmp\\${p.outputBase}")\n`;
  prc += `skip PROGRAM_REPLACE if exists("tmp\\${p.outputBase}${p.outputExt}")\n`;
  prc += `save "tmp\\${p.outputBase}${p.outputExt}"\n`;
  prc += "skip XIT\n";
  prc += ":PROGRAM_REPLACE\n";
  prc += `replace "tmp\\${p.outputBase}${p.outputExt}"\n`;
  prc += "skip XIT\n";
  prc += ":XIT\n";
  prc += "system\n";
  return prc;
}

export function stripAnsi(text: string): string {
  return text.replace(ANSI_RE, "");
}

// BR status line (row 25) layout:
//   Col 1-7:   State (READY, PROC, RUN, etc.)
//   Col 9-37:  Info area (release, error details)
//   Col 62-63: Procedure level (P1, P2, ...)
//   Col 65-73: Serial number
//   Col 75-79: Version
//
// Error formats on the status line:
// Full: 4-digit error + 5-digit line + :NN clause + ERROR + program
//   e.g. "414800430:01ERROR  C:\lexi." → error 4148 on line 430 clause 1
// Short: 3-4 digit error + ERROR (proc-level error, no line number)
//   e.g. "1026ERROR" → error 1026
const BR_ERROR_FULL_RE = /(\d{4})(\d{5}):(\d{2})ERROR\s*(.*)/;
const BR_ERROR_SHORT_RE = /(\d{3,4})ERROR/;
const FATAL_ERROR_RE = /Fatal error called with message:\s*(.*)/;

// State keyword at start of status line
const BR_STATE_RE = /^(READY|PROC|RUN|INPUT|KEYIN|PRINT|STOP|PAUSE|DEBUG|SYNTAX)\b/i;

export function parseBrOutput(output: string): string | null {
  const fatal = FATAL_ERROR_RE.exec(output);
  if (fatal) {
    return fatal[1].trim();
  }
  const full = BR_ERROR_FULL_RE.exec(output);
  if (full) {
    const errorCode = parseInt(full[1], 10);
    const lineNum = parseInt(full[2], 10);
    const clause = parseInt(full[3], 10);
    const program = full[4].trim().replace(/\.$/, "");
    let msg = `Error ${errorCode} at line ${lineNum}`;
    if (clause > 0) {
      msg += `:${clause}`;
    }
    if (program) {
      msg += ` in ${program}`;
    }
    return msg;
  }
  const short = BR_ERROR_SHORT_RE.exec(output);
  if (short) {
    return `Error ${parseInt(short[1], 10)}`;
  }
  return null;
}

export function parseBrState(output: string): string | null {
  const match = BR_STATE_RE.exec(output);
  return match ? match[1].toUpperCase() : null;
}

const COMPILE_TIMEOUT_MS = 30_000;

function runBrLinux(brlinuxPath: string, lexiPath: string, prcFile: string): Promise<void> {
  // brlinux requires a PTY — use `script` to allocate one.
  // We manage stdin ourselves to respond to prompts instead of blindly piping.
  const cmd = `script -qc "LD_LIBRARY_PATH='${lexiPath}' ${LOADER} '${brlinuxPath}' proc ${prcFile}" /dev/null`;

  return new Promise((resolve, reject) => {
    const proc = spawn("bash", ["-c", cmd], {
      cwd: lexiPath,
      stdio: "pipe",
    });

    let stdout = "";
    let lastChecked = 0;
    let settled = false;

    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        proc.kill();
        reject(new Error("Compilation timed out after 30s"));
      }
    }, COMPILE_TIMEOUT_MS);

    function settle(err?: Error) {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    }

    proc.stdout?.on("data", (data: Buffer) => {
      const chunk = data.toString();
      rawOutputChannel.append(chunk);
      stdout += chunk;
      const cleaned = stripAnsi(stdout);
      const recent = cleaned.slice(lastChecked);

      // "Too many users" — send E to exit, don't retry
      if (/E to exit/i.test(recent)) {
        lastChecked = cleaned.length;
        proc.stdin?.write("E");
        return;
      }

      // BR error on status line (e.g. "1026ERROR", "4148...ERROR") —
      // Kill the process immediately; BR's proc state after an error is
      // unpredictable and sending `system` often gets echoed without effect.
      if (/\d{3,4}ERROR/i.test(recent)) {
        const errMsg = parseBrOutput(cleaned) ?? "Unknown BR error";
        proc.kill();
        settle(new Error(errMsg));
        return;
      }

      // Dismiss "Press any key" prompts (startup license screen, error dismissal)
      if (/Press any key/i.test(recent)) {
        lastChecked = cleaned.length;
        proc.stdin?.write("\n");
      }
    });

    proc.on("close", (code) => {
      const brError = parseBrOutput(stripAnsi(stdout));
      if (brError) {
        settle(new Error(brError));
      } else if (code !== 0) {
        settle(new Error(`brlinux exited with code ${code}`));
      } else {
        settle();
      }
    });

    proc.on("error", (err) => settle(err));
  });
}

function runBrWindows(brExePath: string, lexiPath: string, prcFile: string): Promise<void> {
  // Launch LexiTip to auto-dismiss the BR splash/license screen
  const lexiTipPath = path.join(lexiPath, "LexiTip.exe");
  if (fs.existsSync(lexiTipPath)) {
    spawn(lexiTipPath, [], { cwd: lexiPath, detached: true, stdio: "ignore" }).unref();
  }

  return new Promise((resolve, reject) => {
    const proc = spawn(brExePath, ["proc", prcFile], {
      cwd: lexiPath,
      stdio: "pipe",
    });

    let stdout = "";
    let stderr = "";
    let settled = false;

    const timeout = setTimeout(() => {
      if (!settled) {
        settled = true;
        proc.kill();
        reject(new Error("Compilation timed out after 30s"));
      }
    }, COMPILE_TIMEOUT_MS);

    function settle(err?: Error) {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (err) {
        reject(err);
      } else {
        resolve();
      }
    }

    proc.stdout?.on("data", (data: Buffer) => {
      const chunk = data.toString();
      rawOutputChannel.append(chunk);
      stdout += chunk;
    });

    proc.stderr?.on("data", (data: Buffer) => {
      rawOutputChannel.append(data.toString());
      stderr += data.toString();
    });

    proc.on("close", (code) => {
      const brError = parseBrOutput(stripAnsi(stdout));
      if (brError) {
        settle(new Error(brError));
      } else if (code !== 0) {
        settle(new Error(stderr.trim() || `brnative.exe exited with code ${code}`));
      } else {
        settle();
      }
    });

    proc.on("error", (err) => settle(err));
  });
}

function runBr(context: vscode.ExtensionContext, lexiPath: string, prcFile: string): Promise<void> {
  if (process.platform === "win32") {
    const brExePath = path.join(lexiPath, "brnative.exe");
    if (!fs.existsSync(brExePath)) {
      return Promise.reject(new Error("brnative.exe not found in Lexi/ directory. Please add the Windows BR runtime."));
    }
    return runBrWindows(brExePath, lexiPath, prcFile);
  } else {
    const brlinuxPath = path.join(lexiPath, "brlinux");
    if (!fs.existsSync(brlinuxPath)) {
      return Promise.reject(new Error("brlinux not found in Lexi/ directory. Please add the Linux BR runtime."));
    }
    ensureExecutable(brlinuxPath);
    return runBrLinux(brlinuxPath, lexiPath, prcFile);
  }
}

export async function compileBrProgram(filename: string, context: vscode.ExtensionContext, focusOutput: boolean): Promise<boolean> {
  const parsed = path.parse(filename);
  const inputExt = parsed.ext.toLowerCase();
  const outputExt = EXT_MAP[inputExt];
  if (!outputExt) {
    vscode.window.showErrorMessage(`Unsupported file extension: ${inputExt}`);
    return false;
  }

  // Unique tag per compile to avoid collisions between concurrent compiles
  const tag = String(compileId++);

  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");
  const prcFileName = `compile${tag}.prc`;
  const prcPath = path.join(lexiPath, prcFileName);
  const tmpSourceBase = `${tag}_${parsed.base}`;
  const tmpSourcePath = path.join(tmpDir, tmpSourceBase);
  const tempFileName = `temp${tag}`;
  const tmpOutputBase = `${tag}_${parsed.name}`;
  const outputFileName = parsed.name + outputExt;
  const finalOutputPath = path.join(parsed.dir, outputFileName);

  // Ensure tmp directory exists
  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  // Copy source file to tmp directory
  try {
    fs.copyFileSync(filename, tmpSourcePath);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to copy source file: ${error.message}`);
    return false;
  }

  const hasNumbers = hasLineNumbers(tmpSourcePath);
  const startTime = Date.now();

  rawOutputChannel.appendLine(`\n--- Compiling ${parsed.base} ---`);

  outputChannel.appendLine("");
  outputChannel.appendLine(`Compiling ${parsed.base}`);
  outputChannel.appendLine(`  Source: ${filename}`);
  outputChannel.appendLine(`  Output: ${finalOutputPath}`);
  outputChannel.appendLine(`  Lexi: ${hasNumbers ? "preprocessing only" : "preprocessing + line numbers"}`);
  outputChannel.show(!focusOutput);

  // Generate and write .prc file
  const prcContent = generatePrc({
    sourceBase: tmpSourceBase,
    tempFile: tempFileName,
    outputBase: tmpOutputBase,
    outputExt,
    hasNumbers,
  });
  try {
    fs.writeFileSync(prcPath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create procedure file: ${error.message}`);
    return false;
  }

  try {
    await runBr(context, lexiPath, prcFileName);
  } catch (error: any) {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: ${error.message}`);
    outputChannel.show(false);
    vscode.window.showErrorMessage(`Compilation failed: ${error.message}`);
    return false;
  } finally {
    // Clean up .prc file and Lexi tempfile
    for (const f of [prcPath, path.join(tmpDir, tempFileName)]) {
      try {
        if (fs.existsSync(f)) {
          fs.unlinkSync(f);
        }
      } catch {
        // ignore
      }
    }
  }

  // Copy compiled file back — try with extension first, then without
  const tmpCompiledPath = path.join(tmpDir, tmpOutputBase + outputExt);
  const tmpCompiledNoExt = path.join(tmpDir, tmpOutputBase);

  let sourceFile: string | null = null;
  if (fs.existsSync(tmpCompiledPath)) {
    sourceFile = tmpCompiledPath;
  } else if (fs.existsSync(tmpCompiledNoExt)) {
    sourceFile = tmpCompiledNoExt;
  }

  if (sourceFile) {
    try {
      fs.copyFileSync(sourceFile, finalOutputPath);
      fs.unlinkSync(sourceFile);
      const elapsed = Date.now() - startTime;
      outputChannel.appendLine(`  Result: OK (${elapsed}ms)`);
    } catch (copyError: any) {
      vscode.window.showErrorMessage(`Failed to copy compiled file: ${copyError.message}`);
      return false;
    }
  } else {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: compiled file not found in tmp directory`);
    outputChannel.show(false);
    vscode.window.showErrorMessage("Compiled file not found in tmp directory");
    return false;
  }

  // Clean up source file from tmp
  try {
    if (fs.existsSync(tmpSourcePath)) {
      fs.unlinkSync(tmpSourcePath);
    }
  } catch {
    // ignore
  }

  return true;
}

function toggleAutoCompile(): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "br") {
    return;
  }
  const fileName = editor.document.fileName;
  const current = autoCompileState.get(fileName) ?? false;
  autoCompileState.set(fileName, !current);
  updateStatusBar();
}

function updateStatusBar(): void {
  const editor = vscode.window.activeTextEditor;
  if (!editor || editor.document.languageId !== "br") {
    autoCompileStatusBarItem.hide();
    return;
  }
  const enabled = autoCompileState.get(editor.document.fileName) ?? false;
  autoCompileStatusBarItem.text = enabled ? "$(check) Auto-Compile" : "$(circle-slash) Auto-Compile";
  autoCompileStatusBarItem.show();
}

export function activateCompile(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel("BR Compile");
  rawOutputChannel = vscode.window.createOutputChannel("BR Compile (Raw)");

  autoCompileStatusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  autoCompileStatusBarItem.command = "br-lsp.toggleAutoCompile";
  updateStatusBar();

  context.subscriptions.push(
    outputChannel,
    rawOutputChannel,
    autoCompileStatusBarItem,
    vscode.commands.registerCommand("br-lsp.compile", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage("No active editor");
        return;
      }

      const filename = editor.document.fileName;
      const ext = path.extname(filename).toLowerCase();
      if (!EXT_MAP[ext]) {
        vscode.window.showErrorMessage("Current file is not a BR source file (.brs or .wbs)");
        return;
      }

      // Save the file before compiling
      if (editor.document.isDirty) {
        await editor.document.save();
      }

      await compileBrProgram(filename, context, true);
    }),
    vscode.commands.registerCommand("br-lsp.toggleAutoCompile", toggleAutoCompile),
    vscode.window.onDidChangeActiveTextEditor(() => updateStatusBar()),
    vscode.workspace.onDidSaveTextDocument(async (document) => {
      if (document.languageId !== "br") {
        return;
      }
      if (!autoCompileState.get(document.fileName)) {
        return;
      }
      if (compiling) {
        return;
      }
      compiling = true;
      try {
        await compileBrProgram(document.fileName, context, false);
      } finally {
        compiling = false;
      }
    }),
  );
}
