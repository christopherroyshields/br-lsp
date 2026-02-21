import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { getLexiPath, runBr } from "./compile";

const DECOMPILE_EXT_MAP: Record<string, string> = {
  ".br": ".brs",
  ".wb": ".wbs",
};

let outputChannel: vscode.OutputChannel;
let decompileId = 0;

async function decompileBrProgram(
  compiledFile: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  const parsed = path.parse(compiledFile);
  const inputExt = parsed.ext.toLowerCase();
  const outputExt = DECOMPILE_EXT_MAP[inputExt];
  if (!outputExt) {
    vscode.window.showErrorMessage(`Unsupported file extension: ${inputExt}`);
    return;
  }

  const finalOutputPath = path.join(parsed.dir, parsed.name + outputExt);

  // Prompt to overwrite if source already exists
  if (fs.existsSync(finalOutputPath)) {
    const overwrite = await vscode.window.showWarningMessage(
      `${parsed.name}${outputExt} already exists. Overwrite?`,
      "Overwrite",
      "Cancel",
    );
    if (overwrite !== "Overwrite") {
      return;
    }
  }

  const tag = String(decompileId++);
  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");
  const prcFileName = `tmp/decompile${tag}.prc`;
  const prcPath = path.join(lexiPath, prcFileName);
  const tmpOutputName = `decompile${tag}${outputExt}`;
  const tmpOutputPath = path.join(tmpDir, tmpOutputName);

  // Ensure tmp directory exists
  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  // Generate proc file: load the compiled program, then list > to decompile
  const prcContent = [
    "proc noecho",
    `load ":${compiledFile}"`,
    `list >":${tmpOutputPath}"`,
    "system",
    "",
  ].join("\n");

  const startTime = Date.now();
  outputChannel.appendLine("");
  outputChannel.appendLine(`Decompiling ${parsed.base}`);
  outputChannel.appendLine(`  Source: ${compiledFile}`);
  outputChannel.appendLine(`  Output: ${finalOutputPath}`);
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
    vscode.window.showErrorMessage(`Decompilation failed: ${error.message}`);
    return;
  } finally {
    try {
      if (fs.existsSync(prcPath)) {
        fs.unlinkSync(prcPath);
      }
    } catch {
      // ignore
    }
  }

  // Copy decompiled file to final location
  if (fs.existsSync(tmpOutputPath)) {
    try {
      fs.copyFileSync(tmpOutputPath, finalOutputPath);
      fs.unlinkSync(tmpOutputPath);
      const elapsed = Date.now() - startTime;
      outputChannel.appendLine(`  Result: OK (${elapsed}ms)`);
    } catch (error: any) {
      vscode.window.showErrorMessage(`Failed to copy decompiled file: ${error.message}`);
      return;
    }
  } else {
    const elapsed = Date.now() - startTime;
    outputChannel.appendLine(`  Result: FAILED (${elapsed}ms)`);
    outputChannel.appendLine(`  Error: decompiled file not found in tmp directory`);
    outputChannel.show(false);
    vscode.window.showErrorMessage("Decompiled file not found in tmp directory");
    return;
  }

  // Open the decompiled file in editor
  const doc = await vscode.workspace.openTextDocument(finalOutputPath);
  await vscode.window.showTextDocument(doc);
}

export function activateDecompile(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel("BR Decompile");

  context.subscriptions.push(
    outputChannel,
    vscode.commands.registerCommand("br-lsp.decompile", async (uri?: vscode.Uri) => {
      let filename: string;
      if (uri) {
        filename = uri.fsPath;
      } else {
        const editor = vscode.window.activeTextEditor;
        if (!editor) {
          vscode.window.showErrorMessage("No file selected");
          return;
        }
        filename = editor.document.fileName;
      }

      const ext = path.extname(filename).toLowerCase();
      if (!DECOMPILE_EXT_MAP[ext]) {
        vscode.window.showErrorMessage("Selected file is not a compiled BR program (.br or .wb)");
        return;
      }

      await decompileBrProgram(filename, context);
    }),
  );
}
