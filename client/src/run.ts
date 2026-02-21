import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { spawn } from "child_process";
import {
  compileBrProgram,
  getLexiPath,
  EXT_MAP,
  stripAnsi,
  parseBrOutput,
  parseBrState,
} from "./compile";

const LOADER = "/lib64/ld-linux-x86-64.so.2";
const ACTIVE_LAUNCH_CONFIG_KEY = "br.activeLaunchConfig";

interface BrLaunchConfiguration {
  type: "br";
  request: "launch";
  name: string;
  executable?: string;
  wbconfig?: string;
  wsid?: string;
  cwd?: string;
}

function getDefaultLaunchConfig(): BrLaunchConfiguration {
  const executable =
    process.platform === "win32"
      ? "${extensionPath}/Lexi/brnative.exe"
      : "${extensionPath}/Lexi/brlinux";
  return {
    type: "br",
    request: "launch",
    name: "BR Default",
    executable,
    wbconfig: "",
    wsid: "",
    cwd: "${fileDirname}",
  };
}

function loadLaunchConfigurations(): BrLaunchConfiguration[] {
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];
  if (!workspaceFolder) {
    return [getDefaultLaunchConfig()];
  }

  const launchJsonPath = path.join(workspaceFolder.uri.fsPath, ".vscode", "launch.json");
  if (!fs.existsSync(launchJsonPath)) {
    return [getDefaultLaunchConfig()];
  }

  try {
    const content = fs.readFileSync(launchJsonPath, "utf8");
    // Remove single-line comments from JSON
    const cleanContent = content.replace(/\/\/.*$/gm, "");
    const launchJson = JSON.parse(cleanContent);

    const brConfigs = ((launchJson.configurations || []) as any[]).filter(
      (config) => config.type === "br",
    ) as BrLaunchConfiguration[];

    return brConfigs.length > 0
      ? [getDefaultLaunchConfig(), ...brConfigs]
      : [getDefaultLaunchConfig()];
  } catch {
    return [getDefaultLaunchConfig()];
  }
}

function resolveVariables(
  str: string,
  activeFile: string | undefined,
  context: vscode.ExtensionContext,
): string {
  const workspaceFolder = vscode.workspace.workspaceFolders?.[0];

  let resolved = str;

  if (workspaceFolder) {
    resolved = resolved.replace(/\$\{workspaceFolder\}/g, workspaceFolder.uri.fsPath);
  }

  resolved = resolved.replace(/\$\{extensionPath\}/g, context.extensionPath);

  if (activeFile) {
    resolved = resolved.replace(/\$\{file\}/g, activeFile);
    resolved = resolved.replace(/\$\{fileDirname\}/g, path.dirname(activeFile));
    resolved = resolved.replace(/\$\{fileBasename\}/g, path.basename(activeFile));
    const parsedPath = path.parse(activeFile);
    resolved = resolved.replace(/\$\{fileBasenameNoExtension\}/g, parsedPath.name);
  }

  if (workspaceFolder && !path.isAbsolute(resolved)) {
    resolved = path.join(workspaceFolder.uri.fsPath, resolved);
  }

  return path.normalize(resolved);
}

async function selectLaunchConfiguration(
  context: vscode.ExtensionContext,
): Promise<BrLaunchConfiguration | undefined> {
  const configs = loadLaunchConfigurations();

  if (configs.length === 0) {
    vscode.window.showErrorMessage("No BR launch configurations found");
    return undefined;
  }

  const items = configs.map((config) => ({
    label: config.name,
    description: config.executable || "",
    config,
  }));

  const selected = await vscode.window.showQuickPick(items, {
    placeHolder: "Select a BR launch configuration",
  });

  if (selected) {
    await context.workspaceState.update(ACTIVE_LAUNCH_CONFIG_KEY, selected.config.name);
    vscode.window.showInformationMessage(`Active launch configuration: ${selected.config.name}`);
    return selected.config;
  }

  return undefined;
}

// Matches cursor positioning to row 25: \x1b[25;Nc H
const ROW25_RE = /\x1b\[25;\d*H/;
// Matches cursor positioning to any other row: \x1b[N;Nc H where N != 25
const ROW_OTHER_RE = /\x1b\[(?!25;)\d+;\d*H/;

function generateWbconfig(lexiPath: string, wbconfig: string, cwd: string): string {
  // Read the base wbconfig (user-specified or default from Lexi)
  const baseConfig = wbconfig || path.join(lexiPath, "wbconfig.sys");
  let content: string;
  try {
    content = fs.readFileSync(baseConfig, "utf8");
  } catch {
    content = "drive c:,.,\\,\\\nwbserver c:\\\n";
  }

  // Replace or prepend the drive line with cwd path
  const driveLine = `DRIVE c,${cwd},x,\\`;
  if (/^\s*drive\s/im.test(content)) {
    content = content.replace(/^\s*drive\s.*/im, driveLine);
  } else {
    content = driveLine + "\n" + content;
  }

  // Also set wbserver to current directory (Lexi, where BR launches from)
  content = content.replace(/^\s*wbserver\s.*/im, "wbserver .");

  const outName = "run.sys";
  fs.writeFileSync(path.join(lexiPath, outName), content);
  return outName;
}

function launchBrTerminal(
  executable: string,
  prcFile: string,
  config: { wsid: string; wbconfig: string; cwd: string },
  lexiPath: string,
): void {
  // Dispose existing "BR Run" terminal
  for (const term of vscode.window.terminals) {
    if (term.name === "BR Run") {
      term.dispose();
    }
  }

  if (process.platform === "win32") {
    // Launch LexiTip to auto-dismiss BR splash/license screen
    const lexiTipPath = path.join(lexiPath, "LexiTip.exe");
    if (fs.existsSync(lexiTipPath)) {
      spawn(lexiTipPath, [], { cwd: lexiPath, detached: true, stdio: "ignore" }).unref();
    }

    const terminal = vscode.window.createTerminal({
      name: "BR Run",
      cwd: lexiPath,
    });

    let cmd = `"${executable}" proc ${prcFile}`;
    if (config.wsid) {
      if (config.wsid.startsWith("+")) {
        cmd += ` ${config.wsid}`;
      } else if (config.wsid === "WSIDCLEAR") {
        cmd += ` -WSIDCLEAR`;
      } else {
        cmd += ` -${config.wsid}`;
      }
    }
    if (config.wbconfig) {
      cmd += ` -${path.basename(config.wbconfig)}`;
    }

    terminal.sendText(cmd);
    terminal.show();
  } else {
    launchBrLinuxTerminal(executable, prcFile, lexiPath, config.wbconfig);
  }
}

let brStatusBarItem: vscode.StatusBarItem | undefined;

function updateBrStatus(state: string): void {
  if (!brStatusBarItem) {
    brStatusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  }
  brStatusBarItem.text = `$(terminal) BR: ${state}`;
  brStatusBarItem.show();
}

function hideBrStatus(): void {
  brStatusBarItem?.hide();
}

function launchBrLinuxTerminal(executable: string, prcFile: string, lexiPath: string, wbconfig: string): void {
  const writeEmitter = new vscode.EventEmitter<string>();
  const closeEmitter = new vscode.EventEmitter<number | void>();

  let proc: ReturnType<typeof spawn> | null = null;
  let row25buf = "";
  let onRow25 = false;

  function checkRow25(): void {
    if (!row25buf) return;
    const clean = stripAnsi(row25buf);

    const err = parseBrOutput(clean);
    if (err) {
      vscode.window.showErrorMessage(`BR: ${err}`);
      updateBrStatus("ERROR");
    }

    const state = parseBrState(clean);
    if (state) {
      updateBrStatus(state);
    }

    row25buf = "";
  }

  const pty: vscode.Pseudoterminal = {
    onDidWrite: writeEmitter.event,
    onDidClose: closeEmitter.event,

    open(dims: vscode.TerminalDimensions | undefined): void {
      const cols = dims?.columns ?? 80;
      const rows = dims?.rows ?? 25;

      const cmd =
        `stty cols ${cols} rows ${rows} erase ^?; ` +
        `LD_LIBRARY_PATH='${lexiPath}' TERM=xterm-256 ` +
        `${LOADER} '${executable}' "proc :${path.join(lexiPath, prcFile)}" -${wbconfig}`;

      proc = spawn("script", ["-qc", cmd, "/dev/null"], {
        cwd: lexiPath,
        env: { ...process.env, TERM: "xterm-256" },
        stdio: "pipe",
      });

      proc.stdout?.on("data", (data: Buffer) => {
        const text = data.toString("binary");

        // Scan for row 25 cursor positioning to capture status line content
        let remaining = text;
        while (remaining.length > 0) {
          if (onRow25) {
            // Check if cursor moves off row 25
            const offMatch = ROW_OTHER_RE.exec(remaining);
            if (offMatch) {
              row25buf += remaining.slice(0, offMatch.index);
              checkRow25();
              onRow25 = false;
              remaining = remaining.slice(offMatch.index);
            } else {
              // Check if another row 25 position resets the buffer
              const r25Match = ROW25_RE.exec(remaining);
              if (r25Match) {
                row25buf += remaining.slice(0, r25Match.index);
                checkRow25();
                row25buf = "";
                remaining = remaining.slice(r25Match.index + r25Match[0].length);
                // still on row 25
              } else {
                row25buf += remaining;
                remaining = "";
              }
            }
          } else {
            const r25Match = ROW25_RE.exec(remaining);
            if (r25Match) {
              remaining = remaining.slice(r25Match.index + r25Match[0].length);
              onRow25 = true;
              row25buf = "";
            } else {
              remaining = "";
            }
          }
        }

        // Forward all data to the terminal display
        writeEmitter.fire(text);
      });

      proc.stderr?.on("data", (data: Buffer) => {
        writeEmitter.fire(data.toString("binary"));
      });

      proc.on("close", (code) => {
        checkRow25();
        hideBrStatus();
        closeEmitter.fire(code ?? undefined);
      });
    },

    close(): void {
      proc?.kill();
      hideBrStatus();
    },

    handleInput(data: string): void {
      // VS Code sends \x7f (DEL) for backspace; BR expects \x08 (BS)
      proc?.stdin?.write(data.replaceAll("\x7f", "\x08"));
    },

    setDimensions(_dims: vscode.TerminalDimensions): void {
      // Send SIGWINCH-equivalent resize via stty on the child's PTY
      // The script command handles this automatically
    },
  };

  const terminal = vscode.window.createTerminal({ name: "BR Run", pty });
  terminal.show();
}

async function runBrProgram(
  activeFilename: string,
  context: vscode.ExtensionContext,
): Promise<void> {
  // Get active configuration or prompt to select
  const activeConfigName = context.workspaceState.get<string>(ACTIVE_LAUNCH_CONFIG_KEY);
  const configs = loadLaunchConfigurations();

  let selectedConfig: BrLaunchConfiguration | undefined;

  if (activeConfigName) {
    selectedConfig = configs.find((c) => c.name === activeConfigName);
  }

  if (!selectedConfig) {
    selectedConfig = await selectLaunchConfiguration(context);
    if (!selectedConfig) {
      return;
    }
  }

  // Resolve variables in configuration
  const executable = resolveVariables(
    selectedConfig.executable || getDefaultLaunchConfig().executable!,
    activeFilename,
    context,
  );
  const wbconfig = selectedConfig.wbconfig
    ? resolveVariables(selectedConfig.wbconfig, activeFilename, context)
    : "";
  const cwd = resolveVariables(selectedConfig.cwd || "${fileDirname}", activeFilename, context);
  const wsid = selectedConfig.wsid || "";

  // Validate executable exists
  if (!fs.existsSync(executable)) {
    const retry = await vscode.window.showErrorMessage(
      `BR executable not found: ${executable}`,
      "Select Different Configuration",
      "Cancel",
    );
    if (retry === "Select Different Configuration") {
      await selectLaunchConfiguration(context);
    }
    return;
  }

  // Save the file before compiling
  const editor = vscode.window.activeTextEditor;
  if (editor && editor.document.fileName === activeFilename && editor.document.isDirty) {
    await editor.document.save();
  }

  // Compile the program
  const ok = await compileBrProgram(activeFilename, context, true);
  if (!ok) {
    return;
  }

  // Check compiled output exists
  const parsedPath = path.parse(activeFilename);
  const inputExt = parsedPath.ext.toLowerCase();
  const outputExt = EXT_MAP[inputExt];
  if (!outputExt) {
    return;
  }
  const compiledProgram = path.join(parsedPath.dir, parsedPath.name + outputExt);

  if (!fs.existsSync(compiledProgram)) {
    vscode.window.showErrorMessage("Compiled program not found. Compilation may have failed.");
    return;
  }

  // Write run.prc in the Lexi directory; BR finds it via absolute path with : prefix
  const lexiPath = getLexiPath(context);
  const prcContent = `proc noecho\nload ":${compiledProgram}"\nrun\n`;
  const prcFileName = "run.prc";
  const prcFilePath = path.join(lexiPath, prcFileName);

  try {
    fs.writeFileSync(prcFilePath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create run procedure: ${error.message}`);
    return;
  }

  // Generate a dynamic wbconfig with the drive pointing to cwd
  const wbconfigName = generateWbconfig(lexiPath, wbconfig, cwd);

  launchBrTerminal(executable, prcFileName, { wsid, wbconfig: wbconfigName, cwd }, lexiPath);
}

export function activateRun(context: vscode.ExtensionContext) {
  context.subscriptions.push(
    vscode.commands.registerCommand("br-lsp.run", async () => {
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

      await runBrProgram(filename, context);
    }),
    vscode.commands.registerCommand("br-lsp.selectLaunchConfig", async () => {
      await selectLaunchConfiguration(context);
    }),
  );
}
