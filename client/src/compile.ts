import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { spawn } from "child_process";

const LOADER = "/lib64/ld-linux-x86-64.so.2";
// Strip ANSI escape sequences and terminal control codes from PTY output
const ANSI_RE = /\x1b\[[0-9;]*[A-Za-z]|\x1b[()][0-9A-Za-z]|\x1b\[\?[0-9;]*[A-Za-z]/g;

let outputChannel: vscode.OutputChannel;
let autoCompileState: Map<string, boolean> = new Map();
let autoCompileStatusBarItem: vscode.StatusBarItem;
let compiling = false;

const EXT_MAP: Record<string, string> = {
  ".brs": ".br",
  ".wbs": ".wb",
};

function getLexiPath(context: vscode.ExtensionContext): string {
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

function generatePrc(sourceBase: string, name: string, outputExt: string, needsLineNumbers: boolean): string {
  // BR uses backslash paths internally even on Linux
  let prc = "";
  prc += "proc noecho\n";
  if (needsLineNumbers) {
    prc += `00002 Infile$="tmp\\${sourceBase}"\n`;
    prc += `00003 Outfile$="tmp\\tempfile"\n`;
    prc += "subproc linenum.brs\n";
    prc += "run\n";
    prc += "clear\n";
    prc += "subproc tmp\\tempfile\n";
  } else {
    prc += `subproc tmp\\${sourceBase}\n`;
  }
  prc += `skip PROGRAM_REPLACE if exists("tmp\\${name}")\n`;
  prc += `skip PROGRAM_REPLACE if exists("tmp\\${name}${outputExt}")\n`;
  prc += `save "tmp\\${name}${outputExt}"\n`;
  prc += "skip XIT\n";
  prc += ":PROGRAM_REPLACE\n";
  prc += `replace "tmp\\${name}${outputExt}"\n`;
  prc += "skip XIT\n";
  prc += ":XIT\n";
  prc += "system\n";
  return prc;
}

function stripAnsi(text: string): string {
  return text.replace(ANSI_RE, "");
}

function runBrLinux(brlinuxPath: string, lexiPath: string): Promise<void> {
  // brlinux requires a PTY — use `script` to allocate one.
  // Pipe a newline to dismiss the startup "Press any key" license screen.
  const cmd = `echo | script -qc "LD_LIBRARY_PATH='${lexiPath}' ${LOADER} '${brlinuxPath}' proc convert.prc" /dev/null`;

  return new Promise((resolve, reject) => {
    const proc = spawn("bash", ["-c", cmd], {
      cwd: lexiPath,
      stdio: "pipe",
    });

    let stderr = "";

    proc.stdout?.on("data", (data: Buffer) => {
      const cleaned = stripAnsi(data.toString())
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
        .join("\n");
      if (cleaned) {
        outputChannel.appendLine(cleaned);
      }
    });

    proc.stderr?.on("data", (data: Buffer) => {
      const text = data.toString();
      stderr += text;
      outputChannel.appendLine(text);
    });

    proc.on("close", (code) => {
      if (code !== 0) {
        reject(new Error(stderr || `brlinux exited with code ${code}`));
      } else {
        resolve();
      }
    });

    proc.on("error", reject);
  });
}

function runBrWindows(brExePath: string, lexiPath: string): Promise<void> {
  // Launch LexiTip to auto-dismiss the BR splash/license screen
  const lexiTipPath = path.join(lexiPath, "LexiTip.exe");
  if (fs.existsSync(lexiTipPath)) {
    spawn(lexiTipPath, [], { cwd: lexiPath, detached: true, stdio: "ignore" }).unref();
  }

  return new Promise((resolve, reject) => {
    const proc = spawn(brExePath, ["proc", "convert.prc"], {
      cwd: lexiPath,
      stdio: "pipe",
    });

    let stderr = "";

    proc.stdout?.on("data", (data: Buffer) => {
      const cleaned = stripAnsi(data.toString())
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line.length > 0)
        .join("\n");
      if (cleaned) {
        outputChannel.appendLine(cleaned);
      }
    });

    proc.stderr?.on("data", (data: Buffer) => {
      const text = data.toString();
      stderr += text;
      outputChannel.appendLine(text);
    });

    proc.on("close", (code) => {
      if (code !== 0) {
        reject(new Error(stderr || `brnative.exe exited with code ${code}`));
      } else {
        resolve();
      }
    });

    proc.on("error", reject);
  });
}

function runBr(context: vscode.ExtensionContext, lexiPath: string): Promise<void> {
  if (process.platform === "win32") {
    const brExePath = path.join(lexiPath, "brnative.exe");
    if (!fs.existsSync(brExePath)) {
      return Promise.reject(new Error("brnative.exe not found in Lexi/ directory. Please add the Windows BR runtime."));
    }
    return runBrWindows(brExePath, lexiPath);
  } else {
    const brlinuxPath = path.join(lexiPath, "brlinux");
    if (!fs.existsSync(brlinuxPath)) {
      return Promise.reject(new Error("brlinux not found in Lexi/ directory. Please add the Linux BR runtime."));
    }
    ensureExecutable(brlinuxPath);
    return runBrLinux(brlinuxPath, lexiPath);
  }
}

async function compileBrProgram(filename: string, context: vscode.ExtensionContext): Promise<void> {
  const parsed = path.parse(filename);
  const inputExt = parsed.ext.toLowerCase();
  const outputExt = EXT_MAP[inputExt];
  if (!outputExt) {
    vscode.window.showErrorMessage(`Unsupported file extension: ${inputExt}`);
    return;
  }

  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");
  const prcPath = path.join(lexiPath, "convert.prc");
  const tmpSourcePath = path.join(tmpDir, parsed.base);
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
    return;
  }

  // Detect whether source needs line numbers added
  const needsLineNumbers = !hasLineNumbers(tmpSourcePath);

  outputChannel.appendLine(`Compiling ${parsed.base}${needsLineNumbers ? " (adding line numbers)" : ""}...`);
  outputChannel.show(true);

  // Generate and write .prc file
  const prcContent = generatePrc(parsed.base, parsed.name, outputExt, needsLineNumbers);
  try {
    fs.writeFileSync(prcPath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create procedure file: ${error.message}`);
    return;
  }

  try {
    await runBr(context, lexiPath);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Compilation failed: ${error.message}`);
    return;
  } finally {
    // Clean up .prc file
    try {
      if (fs.existsSync(prcPath)) {
        fs.unlinkSync(prcPath);
      }
    } catch {
      // ignore
    }

    // Clean up tempfile
    try {
      const tempFile = path.join(tmpDir, "tempfile");
      if (fs.existsSync(tempFile)) {
        fs.unlinkSync(tempFile);
      }
    } catch {
      // ignore
    }
  }

  // Copy compiled file back — try with extension first, then without
  const tmpCompiledPath = path.join(tmpDir, outputFileName);
  const tmpCompiledNoExt = path.join(tmpDir, parsed.name);

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
      vscode.window.setStatusBarMessage(`Compiled ${outputFileName}`, 3000);
    } catch (copyError: any) {
      vscode.window.showErrorMessage(`Failed to copy compiled file: ${copyError.message}`);
      return;
    }
  } else {
    vscode.window.showErrorMessage("Compiled file not found in tmp directory");
    return;
  }

  // Clean up source file from tmp
  try {
    if (fs.existsSync(tmpSourcePath)) {
      fs.unlinkSync(tmpSourcePath);
    }
  } catch {
    // ignore
  }
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

  autoCompileStatusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  autoCompileStatusBarItem.command = "br-lsp.toggleAutoCompile";
  updateStatusBar();

  context.subscriptions.push(
    outputChannel,
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

      await compileBrProgram(filename, context);
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
        await compileBrProgram(document.fileName, context);
      } finally {
        compiling = false;
      }
    }),
  );
}
