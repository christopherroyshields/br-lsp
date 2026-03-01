import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { EXT_MAP, getLexiPath, runBr } from "./compile";

export const DEFAULT_EXT_MAP: Record<string, string> = {
  ".br": ".brs",
  ".bro": ".brs",
  ".wb": ".wbs",
  ".wbo": ".wbs",
};

function getExtMap(): Record<string, string> {
  const config = vscode.workspace.getConfiguration("br");
  return config.get<Record<string, string>>("decompile.sourceExtensions", DEFAULT_EXT_MAP);
}

function getStyleCommand(): string {
  const config = vscode.workspace.getConfiguration("br");
  return config.get<string>("decompile.styleCommand", "indent 2 45 keywords lower labels mixed comments mixed");
}

export function generateDecompilePrc(compiledFile: string, tmpOutputPath: string, styleCmd: string): string {
  const prcLines = [
    "proc noecho",
    `load ":${compiledFile}"`,
  ];
  if (styleCmd) {
    prcLines.push(`style ${styleCmd}`);
  }
  prcLines.push(`list >":${tmpOutputPath}"`);
  prcLines.push("system");
  prcLines.push("");
  return prcLines.join("\n");
}

export function generateBatchDecompilePrc(
  files: { compiledPath: string; tmpOutputPath: string }[],
  styleCmd: string,
): string {
  const prcLines = ["proc noecho"];
  for (const file of files) {
    prcLines.push(`load ":${file.compiledPath}"`);
    if (styleCmd) {
      prcLines.push(`style ${styleCmd}`);
    }
    prcLines.push(`list >":${file.tmpOutputPath}"`);
  }
  prcLines.push("system");
  prcLines.push("");
  return prcLines.join("\n");
}

let outputChannel: vscode.OutputChannel;
let decompileId = 0;

async function decompileBrProgram(
  compiledFile: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  const extMap = getExtMap();
  const parsed = path.parse(compiledFile);
  const inputExt = parsed.ext.toLowerCase();
  const outputExt = extMap[inputExt];
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

  // Generate proc file: load the compiled program, apply style, then list > to decompile
  const prcContent = generateDecompilePrc(compiledFile, tmpOutputPath, getStyleCommand());

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

export function findFilesRecursive(dir: string, extensions: string[]): string[] {
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

      const extMap = getExtMap();
      const compiledExtensions = Object.keys(extMap);
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
        const outputExt = extMap[inputExt];
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

      // Build single .prc with all load/style/list sequences
      const prcLines = generateBatchDecompilePrc(filesToDecompile, getStyleCommand());

      const prcFileName = `tmp/decompile-batch${tag}.prc`;
      const prcPath = path.join(lexiPath, prcFileName);

      const startTime = Date.now();
      outputChannel.appendLine("");
      outputChannel.appendLine(
        `Batch decompiling ${filesToDecompile.length} file(s) from ${folderPath}`,
      );
      outputChannel.show(true);

      try {
        fs.writeFileSync(prcPath, prcLines);
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

interface CacheEntry {
  content: Uint8Array;
  mtimeMs: number;
}

class BRCompiledFS implements vscode.FileSystemProvider {
  private cache = new Map<string, CacheEntry>();
  private pendingReads = new Map<string, Promise<Uint8Array>>();
  private context: vscode.ExtensionContext;

  private _onDidChangeFile = new vscode.EventEmitter<vscode.FileChangeEvent[]>();
  readonly onDidChangeFile = this._onDidChangeFile.event;

  constructor(context: vscode.ExtensionContext) {
    this.context = context;
  }

  static toVirtualUri(compiledFsPath: string): vscode.Uri {
    const parsed = path.parse(compiledFsPath);
    const ext = parsed.ext.toLowerCase();
    const sourceExt = getExtMap()[ext];
    if (!sourceExt) throw new Error(`Unsupported extension: ${ext}`);
    const virtualPath = path.join(parsed.dir, parsed.name + sourceExt);
    const fileUri = vscode.Uri.file(virtualPath);
    return fileUri.with({ scheme: "br-compiled" });
  }

  static toRealPath(uri: vscode.Uri): string {
    const parsed = path.parse(uri.fsPath);
    const ext = parsed.ext.toLowerCase();
    const compiledExt = EXT_MAP[ext];
    if (!compiledExt) throw new Error(`Unsupported extension: ${ext}`);
    return path.join(parsed.dir, parsed.name + compiledExt);
  }

  watch(): vscode.Disposable {
    return new vscode.Disposable(() => {});
  }

  stat(uri: vscode.Uri): vscode.FileStat {
    const realPath = BRCompiledFS.toRealPath(uri);
    let realStat: fs.Stats;
    try {
      realStat = fs.statSync(realPath);
    } catch {
      throw vscode.FileSystemError.FileNotFound(uri);
    }
    const sourcePath = uri.fsPath;
    const sourceExists = fs.existsSync(sourcePath);
    const fileStat = sourceExists ? fs.statSync(sourcePath) : realStat;
    const cached = this.cache.get(realPath);
    return {
      type: vscode.FileType.File,
      ctime: fileStat.ctimeMs,
      mtime: fileStat.mtimeMs,
      size: cached ? cached.content.byteLength : fileStat.size,
    };
  }

  async readFile(uri: vscode.Uri): Promise<Uint8Array> {
    const realPath = BRCompiledFS.toRealPath(uri);

    // If the source file exists on disk, read it directly (skip decompilation)
    const sourcePath = uri.fsPath;
    if (fs.existsSync(sourcePath)) {
      return new Uint8Array(fs.readFileSync(sourcePath));
    }

    let mtimeMs: number;
    try {
      mtimeMs = fs.statSync(realPath).mtimeMs;
    } catch {
      throw vscode.FileSystemError.FileNotFound(uri);
    }

    const cached = this.cache.get(realPath);
    if (cached && cached.mtimeMs === mtimeMs) {
      return cached.content;
    }

    // Deduplicate concurrent reads
    const pending = this.pendingReads.get(realPath);
    if (pending) return pending;

    const promise = this.doReadFile(uri, realPath, mtimeMs);
    this.pendingReads.set(realPath, promise);
    try {
      return await promise;
    } finally {
      this.pendingReads.delete(realPath);
    }
  }

  private async doReadFile(
    uri: vscode.Uri,
    realPath: string,
    mtimeMs: number,
  ): Promise<Uint8Array> {
    const tag = String(decompileId++);
    const lexiPath = getLexiPath(this.context);
    const tmpDir = path.join(lexiPath, "tmp");
    const parsed = path.parse(uri.fsPath);
    const prcFileName = `tmp/vfs-decompile${tag}.prc`;
    const prcPath = path.join(lexiPath, prcFileName);
    const tmpOutputName = `vfs-decompile${tag}${parsed.ext}`;
    const tmpOutputPath = path.join(tmpDir, tmpOutputName);

    if (!fs.existsSync(tmpDir)) {
      fs.mkdirSync(tmpDir, { recursive: true });
    }

    const prcContent = [
      "proc noecho",
      `load ":${realPath}"`,
      `list >":${tmpOutputPath}"`,
      "system",
      "",
    ].join("\n");

    try {
      fs.writeFileSync(prcPath, prcContent);
    } catch (error: any) {
      throw vscode.FileSystemError.Unavailable(error.message);
    }

    try {
      await runBr(this.context, lexiPath, prcFileName);
    } catch (error: any) {
      outputChannel.appendLine(`Virtual FS decompile failed for ${realPath}: ${error.message}`);
      throw vscode.FileSystemError.Unavailable(error.message);
    } finally {
      try {
        if (fs.existsSync(prcPath)) fs.unlinkSync(prcPath);
      } catch {}
    }

    if (!fs.existsSync(tmpOutputPath)) {
      throw vscode.FileSystemError.Unavailable("Decompiled file not found");
    }

    try {
      const content = new Uint8Array(fs.readFileSync(tmpOutputPath));
      this.cache.set(realPath, { content, mtimeMs });
      return content;
    } finally {
      try {
        fs.unlinkSync(tmpOutputPath);
      } catch {}
    }
  }

  async writeFile(
    uri: vscode.Uri,
    content: Uint8Array,
    _options: { create: boolean; overwrite: boolean },
  ): Promise<void> {
    const realPath = BRCompiledFS.toRealPath(uri);
    const tag = String(decompileId++);
    const lexiPath = getLexiPath(this.context);
    const tmpDir = path.join(lexiPath, "tmp");
    const parsed = path.parse(uri.fsPath);
    const compiledExt = EXT_MAP[parsed.ext.toLowerCase()];
    const prcFileName = `tmp/vfs-compile${tag}.prc`;
    const prcPath = path.join(lexiPath, prcFileName);
    const tmpSourceName = `vfs-compile${tag}${parsed.ext}`;
    const tmpSourcePath = path.join(tmpDir, tmpSourceName);
    const tmpOutputBase = `vfs-compile${tag}`;
    const tmpOutputPath = path.join(tmpDir, tmpOutputBase + compiledExt);
    const tmpOutputNoExt = path.join(tmpDir, tmpOutputBase);

    if (!fs.existsSync(tmpDir)) {
      fs.mkdirSync(tmpDir, { recursive: true });
    }

    try {
      fs.writeFileSync(tmpSourcePath, content);
    } catch (error: any) {
      throw vscode.FileSystemError.Unavailable(error.message);
    }

    // Use relative backslash paths from Lexi dir (subproc is a proc command,
    // not a BR statement, so it doesn't support : absolute path prefix)
    const prcContent = [
      "proc noecho",
      `subproc tmp\\${tmpSourceName}`,
      `save "tmp\\${tmpOutputBase}${compiledExt}"`,
      "system",
      "",
    ].join("\n");

    try {
      fs.writeFileSync(prcPath, prcContent);
    } catch (error: any) {
      throw vscode.FileSystemError.Unavailable(error.message);
    }

    try {
      await runBr(this.context, lexiPath, prcFileName);
    } catch (error: any) {
      vscode.window.showErrorMessage(`Compile failed: ${error.message}`);
      throw vscode.FileSystemError.Unavailable(error.message);
    } finally {
      for (const f of [prcPath, tmpSourcePath]) {
        try {
          if (fs.existsSync(f)) fs.unlinkSync(f);
        } catch {}
      }
    }

    // Copy compiled file from tmp to real destination (BR may save with or without extension)
    const compiledFile = fs.existsSync(tmpOutputPath)
      ? tmpOutputPath
      : fs.existsSync(tmpOutputNoExt)
        ? tmpOutputNoExt
        : null;

    if (!compiledFile) {
      vscode.window.showErrorMessage("Compile failed: compiled file not found in tmp directory");
      throw vscode.FileSystemError.Unavailable("Compiled file not found");
    }

    try {
      fs.copyFileSync(compiledFile, realPath);
    } catch (error: any) {
      vscode.window.showErrorMessage(`Failed to copy compiled file: ${error.message}`);
      throw vscode.FileSystemError.Unavailable(error.message);
    } finally {
      try {
        fs.unlinkSync(compiledFile);
      } catch {}
    }

    // Write source text to disk alongside compiled file
    fs.writeFileSync(uri.fsPath, content);

    // Update cache with written content and new mtime
    try {
      const newMtime = fs.statSync(realPath).mtimeMs;
      this.cache.set(realPath, { content: new Uint8Array(content), mtimeMs: newMtime });
    } catch {}

    this._onDidChangeFile.fire([{ type: vscode.FileChangeType.Changed, uri }]);
  }

  readDirectory(): [string, vscode.FileType][] {
    throw vscode.FileSystemError.NoPermissions();
  }

  createDirectory(): void {
    throw vscode.FileSystemError.NoPermissions();
  }

  delete(): void {
    throw vscode.FileSystemError.NoPermissions();
  }

  rename(): void {
    throw vscode.FileSystemError.NoPermissions();
  }
}

class CompiledBRDocument implements vscode.CustomDocument {
  constructor(public readonly uri: vscode.Uri) {}
  dispose(): void {}
}

class CompiledBREditorProvider implements vscode.CustomReadonlyEditorProvider {
  constructor(private context: vscode.ExtensionContext) {}

  async openCustomDocument(uri: vscode.Uri): Promise<CompiledBRDocument> {
    this.handleCompiledFile(uri);
    return new CompiledBRDocument(uri);
  }

  async resolveCustomEditor(
    _document: CompiledBRDocument,
    webviewPanel: vscode.WebviewPanel,
  ): Promise<void> {
    webviewPanel.webview.html = `
      <html>
        <body style="padding: 20px;">
          <h3>BR Compiled File</h3>
          <p>Opening source file...</p>
        </body>
      </html>
    `;
    setTimeout(() => {
      try {
        webviewPanel.dispose();
      } catch {
        // ignore
      }
    }, 500);
  }

  private async handleCompiledFile(uri: vscode.Uri): Promise<void> {
    const virtualUri = BRCompiledFS.toVirtualUri(uri.fsPath);
    await vscode.commands.executeCommand("vscode.open", virtualUri);
  }
}

class BRCompiledDecorator implements vscode.FileDecorationProvider {
  provideFileDecoration(uri: vscode.Uri): vscode.FileDecoration | undefined {
    if (uri.scheme === "br-compiled") {
      return {
        badge: "O",
        tooltip: "Compiled BR program",
        color: new vscode.ThemeColor("charts.purple"),
      };
    }
  }
}

export function activateDecompile(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel("BR Decompile");

  const compiledFS = new BRCompiledFS(context);
  const editorProvider = new CompiledBREditorProvider(context);
  const decorator = new BRCompiledDecorator();

  // Warn when opening a .brs/.wbs from disk if the corresponding .br/.wb is newer
  const warnedUris = new Set<string>();
  function checkStaleSource(document: vscode.TextDocument): void {
    if (document.uri.scheme !== "file") return;
    const ext = path.extname(document.fileName).toLowerCase();
    const compiledExt = EXT_MAP[ext];
    if (!compiledExt) return;

    const parsed = path.parse(document.fileName);
    const compiledPath = path.join(parsed.dir, parsed.name + compiledExt);

    let compiledMtime: number;
    try {
      compiledMtime = fs.statSync(compiledPath).mtimeMs;
    } catch {
      return; // no compiled file — nothing to warn about
    }

    let sourceMtime: number;
    try {
      sourceMtime = fs.statSync(document.fileName).mtimeMs;
    } catch {
      return;
    }

    if (compiledMtime <= sourceMtime) return;

    const key = document.uri.toString();
    if (warnedUris.has(key)) return;
    warnedUris.add(key);

    const compiledName = parsed.name + compiledExt;
    vscode.window
      .showWarningMessage(
        `${compiledName} is newer than ${parsed.base}. The source file may be out of date.`,
        "Open Compiled",
        "Decompile",
        "Dismiss",
      )
      .then((choice) => {
        if (choice === "Open Compiled") {
          const virtualUri = BRCompiledFS.toVirtualUri(compiledPath);
          vscode.commands.executeCommand("vscode.open", virtualUri);
        } else if (choice === "Decompile") {
          decompileBrProgram(compiledPath, context);
        }
      });
  }

  // Check already-open editors and newly opened documents
  for (const editor of vscode.window.visibleTextEditors) {
    checkStaleSource(editor.document);
  }

  context.subscriptions.push(
    outputChannel,
    vscode.window.registerFileDecorationProvider(decorator),
    vscode.workspace.onDidOpenTextDocument(checkStaleSource),
    vscode.workspace.registerFileSystemProvider("br-compiled", compiledFS, {
      isCaseSensitive: true,
      isReadonly: false,
    }),
    vscode.window.registerCustomEditorProvider("br.compiledBREditor", editorProvider, {
      supportsMultipleEditorsPerDocument: false,
    }),
    vscode.commands.registerCommand("br.decompile", async (uri?: vscode.Uri) => {
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
      if (!getExtMap()[ext]) {
        vscode.window.showErrorMessage("Selected file is not a compiled BR program");
        return;
      }

      await decompileBrProgram(filename, context);
    }),
    vscode.commands.registerCommand("br.decompileFolder", async (uri?: vscode.Uri) => {
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
