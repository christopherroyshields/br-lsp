import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { getLexiPath, runBr } from "./compile";

let outputChannel: vscode.OutputChannel;
let lexiId = 0;

async function addLineNumbers(
  filename: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  const parsed = path.parse(filename);
  const tag = String(lexiId++);
  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");
  const prcFileName = `tmp/addnum${tag}.prc`;
  const prcPath = path.join(lexiPath, prcFileName);
  const sourceBase = `${tag}_${parsed.base}`;
  const sourcePath = path.join(tmpDir, sourceBase);
  const tempFile = `addnum${tag}`;
  const tempPath = path.join(tmpDir, tempFile);

  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  try {
    fs.copyFileSync(filename, sourcePath);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to copy source file: ${error.message}`);
    return;
  }

  // Variable assignments on lines 00025/00026 run after the dim on line 00020 in linenum.brs
  const prcContent = [
    "proc noecho",
    "subproc linenum.brs",
    `00025 Infile$="tmp\\${sourceBase}"`,
    `00026 Outfile$="tmp\\${tempFile}"`,
    "run",
    "system",
    "",
  ].join("\n");

  const startTime = Date.now();
  outputChannel.appendLine("");
  outputChannel.appendLine(`Adding line numbers to ${parsed.base}`);
  outputChannel.show(true);

  try {
    fs.writeFileSync(prcPath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create procedure file: ${error.message}`);
    return;
  }

  try {
    await runBr(context, lexiPath, prcFileName);
  } catch (error: any) {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: ${error.message}`);
    outputChannel.show(false);
    vscode.window.showErrorMessage(`Add line numbers failed: ${error.message}`);
    return;
  } finally {
    for (const f of [prcPath, sourcePath]) {
      try {
        if (fs.existsSync(f)) fs.unlinkSync(f);
      } catch {}
    }
  }

  if (fs.existsSync(tempPath)) {
    try {
      fs.copyFileSync(tempPath, filename);
      fs.unlinkSync(tempPath);
      const elapsed = Date.now() - startTime;
      outputChannel.appendLine(`  Result: OK (${elapsed}ms)`);
    } catch (error: any) {
      vscode.window.showErrorMessage(`Failed to copy result file: ${error.message}`);
      return;
    }
  } else {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: output file not found in tmp directory`);
    outputChannel.show(false);
    vscode.window.showErrorMessage("Output file not found in tmp directory");
  }
}

async function stripLineNumbers(
  filename: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  const parsed = path.parse(filename);
  const tag = String(lexiId++);
  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");
  const prcFileName = `tmp/strip${tag}.prc`;
  const prcPath = path.join(lexiPath, prcFileName);
  const sourceBase = `${tag}_${parsed.base}`;
  const sourcePath = path.join(tmpDir, sourceBase);
  const tempFile = `strip${tag}`;
  const tempPath = path.join(tmpDir, tempFile);

  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  try {
    fs.copyFileSync(filename, sourcePath);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to copy source file: ${error.message}`);
    return;
  }

  // Variable assignments on lines 00102/00103 run after the dim on line 00100 in strip.brs
  const prcContent = [
    "proc noecho",
    "subproc strip.brs",
    `00102 Infile$="tmp\\${sourceBase}"`,
    `00103 Outfile$="tmp\\${tempFile}"`,
    "run",
    "system",
    "",
  ].join("\n");

  const startTime = Date.now();
  outputChannel.appendLine("");
  outputChannel.appendLine(`Stripping line numbers from ${parsed.base}`);
  outputChannel.show(true);

  try {
    fs.writeFileSync(prcPath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create procedure file: ${error.message}`);
    return;
  }

  try {
    await runBr(context, lexiPath, prcFileName);
  } catch (error: any) {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: ${error.message}`);
    outputChannel.show(false);
    vscode.window.showErrorMessage(`Strip line numbers failed: ${error.message}`);
    return;
  } finally {
    for (const f of [prcPath, sourcePath]) {
      try {
        if (fs.existsSync(f)) fs.unlinkSync(f);
      } catch {}
    }
  }

  if (fs.existsSync(tempPath)) {
    try {
      fs.copyFileSync(tempPath, filename);
      fs.unlinkSync(tempPath);
      const elapsed = Date.now() - startTime;
      outputChannel.appendLine(`  Result: OK (${elapsed}ms)`);
    } catch (error: any) {
      vscode.window.showErrorMessage(`Failed to copy result file: ${error.message}`);
      return;
    }
  } else {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: output file not found in tmp directory`);
    outputChannel.show(false);
    vscode.window.showErrorMessage("Output file not found in tmp directory");
  }
}

const BR_EXTENSIONS = [".brs", ".wbs"];

export function activateLexi(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel("BR Lexi");

  context.subscriptions.push(
    outputChannel,
    vscode.commands.registerCommand("br-lsp.addLineNumbers", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage("No active editor");
        return;
      }

      const filename = editor.document.fileName;
      const ext = path.extname(filename).toLowerCase();
      if (!BR_EXTENSIONS.includes(ext)) {
        vscode.window.showErrorMessage("Current file is not a BR source file (.brs or .wbs)");
        return;
      }

      if (editor.document.isDirty) {
        await editor.document.save();
      }

      await addLineNumbers(filename, context);
    }),
    vscode.commands.registerCommand("br-lsp.stripLineNumbers", async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        vscode.window.showErrorMessage("No active editor");
        return;
      }

      const filename = editor.document.fileName;
      const ext = path.extname(filename).toLowerCase();
      if (!BR_EXTENSIONS.includes(ext)) {
        vscode.window.showErrorMessage("Current file is not a BR source file (.brs or .wbs)");
        return;
      }

      if (editor.document.isDirty) {
        await editor.document.save();
      }

      await stripLineNumbers(filename, context);
    }),
  );
}
