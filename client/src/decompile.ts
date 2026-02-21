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

function findFilesRecursive(dir: string, extensions: string[]): string[] {
  const results: string[] = [];
  try {
    const items = fs.readdirSync(dir);
    for (const item of items) {
      const fullPath = path.join(dir, item);
      try {
        const stat = fs.statSync(fullPath);
        if (stat.isDirectory()) {
          results.push(...findFilesRecursive(fullPath, extensions));
        } else if (stat.isFile()) {
          const ext = path.extname(fullPath).toLowerCase();
          if (extensions.includes(ext)) {
            results.push(fullPath);
          }
        }
      } catch {
        continue;
      }
    }
  } catch {
    // skip unreadable directories
  }
  return results;
}

async function decompileFolder(
  folderPath: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  return vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Decompiling BR Programs",
      cancellable: false,
    },
    async (progress) => {
      progress.report({ message: "Scanning for compiled BR files..." });

      const compiledExtensions = Object.keys(DECOMPILE_EXT_MAP);
      const compiledFiles = findFilesRecursive(folderPath, compiledExtensions);

      if (compiledFiles.length === 0) {
        vscode.window.showInformationMessage("No compiled BR files found in folder.");
        return;
      }

      interface FileToDecompile {
        compiledPath: string;
        sourcePath: string;
        tmpOutputPath: string;
      }

      const filesToDecompile: FileToDecompile[] = [];
      let skippedCount = 0;

      const tag = String(decompileId++);
      const lexiPath = getLexiPath(context);
      const tmpDir = path.join(lexiPath, "tmp");

      for (const compiledPath of compiledFiles) {
        const parsed = path.parse(compiledPath);
        const inputExt = parsed.ext.toLowerCase();
        const outputExt = DECOMPILE_EXT_MAP[inputExt];
        if (!outputExt) continue;

        const sourcePath = path.join(parsed.dir, parsed.name + outputExt);

        if (fs.existsSync(sourcePath)) {
          skippedCount++;
          continue;
        }

        // Use index to avoid name collisions from different directories
        const tmpName = `decompile${tag}_${filesToDecompile.length}${outputExt}`;
        filesToDecompile.push({
          compiledPath,
          sourcePath,
          tmpOutputPath: path.join(tmpDir, tmpName),
        });
      }

      if (filesToDecompile.length === 0) {
        vscode.window.showInformationMessage(
          `All ${compiledFiles.length} file(s) already have source files. Nothing to decompile.`,
        );
        return;
      }

      progress.report({ message: `Decompiling ${filesToDecompile.length} file(s)...` });

      if (!fs.existsSync(tmpDir)) {
        fs.mkdirSync(tmpDir, { recursive: true });
      }

      // Build single .prc with all load/list pairs
      const prcLines = ["proc noecho"];
      for (const file of filesToDecompile) {
        prcLines.push(`load ":${file.compiledPath}"`);
        prcLines.push(`list >":${file.tmpOutputPath}"`);
      }
      prcLines.push("system");
      prcLines.push("");

      const prcFileName = `tmp/decompile-batch${tag}.prc`;
      const prcPath = path.join(lexiPath, prcFileName);

      const startTime = Date.now();
      outputChannel.appendLine("");
      outputChannel.appendLine(
        `Batch decompiling ${filesToDecompile.length} file(s) from ${folderPath}`,
      );
      outputChannel.show(true);

      try {
        fs.writeFileSync(prcPath, prcLines.join("\n"));
      } catch (error: any) {
        vscode.window.showErrorMessage(`Failed to create procedure file: ${error.message}`);
        return;
      }

      try {
        await runBr(context, lexiPath, prcFileName);
      } catch (error: any) {
        const elapsed = Date.now() - startTime;
        outputChannel.appendLine(`  Batch decompilation FAILED (${elapsed}ms): ${error.message}`);
        outputChannel.show(false);
        vscode.window.showErrorMessage(`Batch decompilation failed: ${error.message}`);
        return;
      } finally {
        try {
          if (fs.existsSync(prcPath)) fs.unlinkSync(prcPath);
        } catch {
          // ignore
        }
      }

      // Copy decompiled files to final locations
      let successCount = 0;
      let failCount = 0;

      for (const file of filesToDecompile) {
        if (fs.existsSync(file.tmpOutputPath)) {
          try {
            fs.copyFileSync(file.tmpOutputPath, file.sourcePath);
            fs.unlinkSync(file.tmpOutputPath);
            successCount++;
            outputChannel.appendLine(`  OK: ${file.sourcePath}`);
          } catch (error: any) {
            failCount++;
            outputChannel.appendLine(`  FAILED: ${file.sourcePath} — ${error.message}`);
          }
        } else {
          failCount++;
          outputChannel.appendLine(`  FAILED: ${file.sourcePath} — decompiled file not found`);
        }
      }

      const elapsed = Date.now() - startTime;
      let summary = `Decompiled ${successCount} file(s)`;
      if (skippedCount > 0) summary += `, skipped ${skippedCount} existing`;
      if (failCount > 0) summary += `, ${failCount} failed`;
      summary += ` (${elapsed}ms)`;

      outputChannel.appendLine(`  ${summary}`);

      if (failCount > 0) {
        vscode.window.showWarningMessage(summary);
      } else {
        vscode.window.showInformationMessage(summary);
      }
    },
  );
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
    vscode.commands.registerCommand("br-lsp.decompileFolder", async (uri?: vscode.Uri) => {
      let targetFolder: string | undefined;
      if (uri) {
        targetFolder = uri.fsPath;
        if (!fs.existsSync(targetFolder) || !fs.statSync(targetFolder).isDirectory()) {
          vscode.window.showErrorMessage("Please select a folder");
          return;
        }
      } else {
        const result = await vscode.window.showOpenDialog({
          canSelectFiles: false,
          canSelectFolders: true,
          canSelectMany: false,
          openLabel: "Select Folder to Decompile",
        });
        if (result && result.length > 0) {
          targetFolder = result[0].fsPath;
        } else {
          return;
        }
      }
      if (targetFolder) {
        await decompileFolder(targetFolder, context);
      }
    }),
  );
}
