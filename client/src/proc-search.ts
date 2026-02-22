import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { getLexiPath, runBr } from "./compile";

const DECOMPILE_EXT_MAP: Record<string, string> = {
  ".br": ".brs",
  ".wb": ".wbs",
};

let outputChannel: vscode.OutputChannel;
let searchResultsProvider: SearchResultsProvider;
let searchTreeView: vscode.TreeView<vscode.TreeItem>;
let lastSearchTerms: string | undefined;
let searchId = 0;

// --- Search term parsing ---

interface ParseResult {
  terms: string[];
  error?: string;
}

function parseSearchTerms(input: string): ParseResult {
  const terms: string[] = [];
  const pattern = /(~?)(['"])((?:(?!\2).)*)\2/g;
  let match;

  while ((match = pattern.exec(input)) !== null) {
    const notOp = match[1];
    const quote = match[2];
    const term = match[3];

    if (!term) {
      return { terms: [], error: "Empty search term not allowed" };
    }

    terms.push(`${notOp}${quote}${term}${quote}`);
  }

  if (terms.length === 0) {
    return { terms: [], error: "No valid search terms found. Use quoted strings: 'term' or \"term\"" };
  }

  // Check for unparsed content
  const reconstructed = terms.join(" ");
  if (input.replace(/\s+/g, " ").trim() !== reconstructed.replace(/\s+/g, " ").trim()) {
    return { terms: [], error: "Invalid syntax. Use BR LIST format: 'term' \"term\" ~'term'" };
  }

  if (terms.length > 3) {
    return { terms: [], error: "Maximum 3 search terms allowed (BR LIST limitation)" };
  }

  return { terms };
}

function validateSearchInput(value: string): string | null {
  if (!value || value.trim().length === 0) {
    return "Please enter at least one search parameter";
  }
  if (!/['"]/.test(value)) {
    return "Search terms must be quoted. Use 'term' for case-insensitive or \"term\" for case-sensitive";
  }
  const result = parseSearchTerms(value.trim());
  return result.error ?? null;
}

// --- Proc generation ---

function generateSearchPrc(
  files: vscode.Uri[],
  searchTerms: string[],
  tmpDir: string,
  tag: string,
): string {
  const termStr = searchTerms.join(" ");
  const lines: string[] = ["proc noecho", "PROCERR RETURN"];

  for (let i = 0; i < files.length; i++) {
    const filePath = files[i].fsPath;
    const resultFile = path.join(tmpDir, `search${tag}_${i}.txt`);
    lines.push(`load ":${filePath}"`);
    lines.push(`list ${termStr} >":${resultFile}"`);
  }

  lines.push("system");
  lines.push("");
  return lines.join("\n");
}

// --- Result parsing ---

interface MatchInfo {
  lineNumber: number;
  lineContent: string;
}

function parseResultFile(resultPath: string): MatchInfo[] {
  if (!fs.existsSync(resultPath)) {
    return [];
  }

  const content = fs.readFileSync(resultPath, "utf8");
  if (!content.trim()) {
    return [];
  }

  const matches: MatchInfo[] = [];
  for (const line of content.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed) continue;

    const match = trimmed.match(/^(\d+)\s+(.*)$/);
    if (match) {
      matches.push({
        lineNumber: parseInt(match[1], 10),
        lineContent: trimmed,
      });
    }
  }
  return matches;
}

// --- Tree view classes ---

class SearchMatchItem extends vscode.TreeItem {
  constructor(
    public readonly filePath: string,
    public readonly internalLineNumber: number,
    lineContent: string,
    searchTerms: string[],
  ) {
    super("", vscode.TreeItemCollapsibleState.None);

    const highlights: [number, number][] = [];
    for (const term of searchTerms) {
      const termMatch = term.match(/~?(['"])(.*?)\1/);
      if (!termMatch) continue;

      const isNegated = term.startsWith("~");
      if (isNegated) continue;

      const searchText = termMatch[2];
      const isCaseSensitive = term.includes('"');
      const searchIn = isCaseSensitive ? lineContent : lineContent.toLowerCase();
      const searchFor = isCaseSensitive ? searchText : searchText.toLowerCase();

      let idx = 0;
      while ((idx = searchIn.indexOf(searchFor, idx)) !== -1) {
        highlights.push([idx, idx + searchFor.length]);
        idx += searchFor.length;
      }
    }

    if (highlights.length > 0) {
      highlights.sort((a, b) => a[0] - b[0]);
      this.label = { label: lineContent, highlights };
    } else {
      this.label = lineContent;
    }

    this.tooltip = `Line ${internalLineNumber}: ${lineContent}`;
    this.iconPath = new vscode.ThemeIcon("arrow-right");
    this.command = {
      command: "br-lsp.openSearchResult",
      title: "Open Search Result",
      arguments: [filePath, internalLineNumber],
    };
    this.contextValue = "searchMatch";
  }
}

class SearchFileItem extends vscode.TreeItem {
  constructor(
    public readonly filePath: string,
    public readonly matches: SearchMatchItem[],
  ) {
    const workspaceFolder = vscode.workspace.getWorkspaceFolder(vscode.Uri.file(filePath));
    const displayPath = workspaceFolder
      ? path.relative(workspaceFolder.uri.fsPath, filePath)
      : path.basename(filePath);

    super(displayPath, vscode.TreeItemCollapsibleState.Expanded);
    this.tooltip = filePath;
    this.description = `${matches.length} match${matches.length !== 1 ? "es" : ""}`;
    this.iconPath = new vscode.ThemeIcon("file-binary");
    this.contextValue = "searchFile";
  }
}

class SearchResultsProvider implements vscode.TreeDataProvider<vscode.TreeItem> {
  private _onDidChangeTreeData = new vscode.EventEmitter<vscode.TreeItem | undefined | null | void>();
  readonly onDidChangeTreeData = this._onDidChangeTreeData.event;

  private results: SearchFileItem[] = [];

  refresh(results: SearchFileItem[]): void {
    this.results = results;
    this._onDidChangeTreeData.fire();
  }

  clear(): void {
    this.results = [];
    this._onDidChangeTreeData.fire();
  }

  getTreeItem(element: vscode.TreeItem): vscode.TreeItem {
    return element;
  }

  getChildren(element?: vscode.TreeItem): vscode.ProviderResult<vscode.TreeItem[]> {
    if (!element) {
      return this.results;
    }
    if (element instanceof SearchFileItem) {
      return element.matches;
    }
    return [];
  }

  getParent(element: vscode.TreeItem): vscode.ProviderResult<vscode.TreeItem> {
    if (element instanceof SearchMatchItem) {
      for (const fileItem of this.results) {
        if (fileItem.matches.includes(element)) {
          return fileItem;
        }
      }
    }
    return null;
  }
}

// --- Navigation ---

async function openSearchResult(
  filePath: string,
  internalLineNumber: number,
): Promise<void> {
  const ext = path.extname(filePath).toLowerCase();
  const sourceExt = DECOMPILE_EXT_MAP[ext];

  let docUri: vscode.Uri;
  if (sourceExt) {
    // Compiled file — check for on-disk source first
    const parsed = path.parse(filePath);
    const sourcePath = path.join(parsed.dir, parsed.name + sourceExt);

    if (fs.existsSync(sourcePath)) {
      docUri = vscode.Uri.file(sourcePath);
    } else {
      // Use br-compiled: virtual FS for auto-decompile
      docUri = vscode.Uri.from({
        scheme: "br-compiled",
        path: path.join(parsed.dir, parsed.name + sourceExt),
      });
    }
  } else {
    docUri = vscode.Uri.file(filePath);
  }

  const document = await vscode.workspace.openTextDocument(docUri);

  // Find the line containing the internal BR line number
  const linePattern = new RegExp(`^0*${internalLineNumber}\\s`);
  let actualLine = 0;

  for (let i = 0; i < document.lineCount; i++) {
    if (linePattern.test(document.lineAt(i).text.trim())) {
      actualLine = i;
      break;
    }
  }

  const editor = await vscode.window.showTextDocument(document, {
    selection: new vscode.Range(actualLine, 0, actualLine, 0),
    viewColumn: vscode.ViewColumn.One,
    preview: false,
  });

  editor.revealRange(
    new vscode.Range(actualLine, 0, actualLine, 0),
    vscode.TextEditorRevealType.InCenter,
  );
}

// --- Main flow ---

async function executeSearch(
  context: vscode.ExtensionContext,
  prefill?: string,
): Promise<void> {
  const input = await vscode.window.showInputBox({
    prompt: "Enter BR LIST search parameters (up to 3 terms)",
    placeHolder: "e.g., 'LET' or \"OPEN\" or 'LET' \"FNEND\" ~'test'",
    value: prefill,
    validateInput: validateSearchInput,
  });

  if (!input) return;

  const parseResult = parseSearchTerms(input.trim());
  if (parseResult.error) {
    vscode.window.showErrorMessage(`Invalid search parameters: ${parseResult.error}`);
    return;
  }

  const searchTerms = parseResult.terms;
  lastSearchTerms = input;

  await vscode.window.withProgress(
    {
      location: vscode.ProgressLocation.Notification,
      title: "Proc Search",
      cancellable: false,
    },
    async (progress) => {
      progress.report({ message: "Scanning workspace files..." });

      outputChannel.clear();
      outputChannel.appendLine(`Proc Search — Searching for: ${searchTerms.join(", ")}`);
      outputChannel.appendLine("");

      searchResultsProvider.clear();
      await vscode.commands.executeCommand("brSearchResults.focus");

      const lexiPath = getLexiPath(context);
      const files = await vscode.workspace.findFiles("**/*.{br,wb}", "**/node_modules/**");

      // Filter out files inside the Lexi directory
      const filtered = files.filter((f) => !f.fsPath.startsWith(lexiPath));

      if (filtered.length === 0) {
        outputChannel.appendLine("No compiled BR programs found in workspace");
        vscode.window.showInformationMessage("No compiled BR programs found in workspace");
        return;
      }

      outputChannel.appendLine(`Found ${filtered.length} file(s) to search`);
      outputChannel.appendLine("");

      const tag = String(searchId++);
      const tmpDir = path.join(lexiPath, "tmp");
      const prcFileName = `tmp/search${tag}.prc`;
      const prcPath = path.join(lexiPath, prcFileName);

      if (!fs.existsSync(tmpDir)) {
        fs.mkdirSync(tmpDir, { recursive: true });
      }

      // Generate and write .prc
      progress.report({ message: "Running BR search..." });
      const prcContent = generateSearchPrc(filtered, searchTerms, tmpDir, tag);
      fs.writeFileSync(prcPath, prcContent);

      try {
        await runBr(context, lexiPath, prcFileName);
      } catch (error: any) {
        outputChannel.appendLine(`Search execution failed: ${error.message}`);
        // Don't bail — PROCERR RETURN means some files may have succeeded
      } finally {
        try {
          if (fs.existsSync(prcPath)) fs.unlinkSync(prcPath);
        } catch {}
      }

      // Parse results
      progress.report({ message: "Parsing results..." });
      const treeItems: SearchFileItem[] = [];
      let totalMatches = 0;

      for (let i = 0; i < filtered.length; i++) {
        const resultPath = path.join(tmpDir, `search${tag}_${i}.txt`);
        const matches = parseResultFile(resultPath);

        if (matches.length > 0) {
          const matchItems = matches.map(
            (m) => new SearchMatchItem(filtered[i].fsPath, m.lineNumber, m.lineContent, searchTerms),
          );
          treeItems.push(new SearchFileItem(filtered[i].fsPath, matchItems));
          totalMatches += matches.length;
        }

        // Cleanup result file
        try {
          if (fs.existsSync(resultPath)) fs.unlinkSync(resultPath);
        } catch {}
      }

      treeItems.sort((a, b) => a.filePath.localeCompare(b.filePath));
      searchResultsProvider.refresh(treeItems);

      const summary = `Found ${totalMatches} match${totalMatches !== 1 ? "es" : ""} in ${treeItems.length} file(s)`;
      outputChannel.appendLine(summary);
      outputChannel.show(true);

      if (treeItems.length > 0) {
        await searchTreeView.reveal(treeItems[0], { select: false, focus: false, expand: true });
      }
    },
  );
}

// --- Activation ---

export function activateProcSearch(context: vscode.ExtensionContext): void {
  outputChannel = vscode.window.createOutputChannel("Proc Search");

  searchResultsProvider = new SearchResultsProvider();
  searchTreeView = vscode.window.createTreeView("brSearchResults", {
    treeDataProvider: searchResultsProvider,
    showCollapseAll: true,
  });

  context.subscriptions.push(
    outputChannel,
    searchTreeView,
    vscode.commands.registerCommand("br-lsp.procSearch", () => executeSearch(context)),
    vscode.commands.registerCommand("br-lsp.modifyProcSearch", () => executeSearch(context, lastSearchTerms)),
    vscode.commands.registerCommand("br-lsp.clearProcSearch", () => {
      searchResultsProvider.clear();
      outputChannel.clear();
      outputChannel.appendLine("Search results cleared");
      lastSearchTerms = undefined;
    }),
    vscode.commands.registerCommand(
      "br-lsp.openSearchResult",
      (filePath: string, internalLineNumber: number) => openSearchResult(filePath, internalLineNumber),
    ),
  );
}
