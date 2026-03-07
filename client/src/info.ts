import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";
import { getLexiPath, runBr } from "./compile";

let infoId = 0;

function generateInfoPrc(statusFile: string, configFile: string, cwdFile: string): string {
  let prc = "";
  prc += "proc noecho\n";
  prc += `status >"tmp\\${statusFile}"\n`;
  prc += `status config >"tmp\\${configFile}"\n`;
  prc += `execute "open #1: 'tmp\\${cwdFile}',display,output : write #1,using 'form pos 1,cc 300': os_filename$('.') : close #1:"\n`;
  prc += "system\n";
  return prc;
}

async function showRuntimeInfo(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration("br");
  const configuredExe = config.get<string>("executable", "");
  const configuredWbconfig = config.get<string>("wbconfig", "");

  const lexiPath = getLexiPath(context);
  const tmpDir = path.join(lexiPath, "tmp");

  if (!fs.existsSync(tmpDir)) {
    fs.mkdirSync(tmpDir, { recursive: true });
  }

  const tag = String(infoId++);
  const statusFile = `info${tag}_status.txt`;
  const configFile = `info${tag}_config.txt`;
  const cwdFile = `info${tag}_cwd.txt`;
  const prcFileName = `tmp/info${tag}.prc`;
  const prcPath = path.join(lexiPath, prcFileName);
  const statusPath = path.join(tmpDir, statusFile);
  const configPath = path.join(tmpDir, configFile);
  const cwdPath = path.join(tmpDir, cwdFile);

  const prcContent = generateInfoPrc(statusFile, configFile, cwdFile);
  try {
    fs.writeFileSync(prcPath, prcContent);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to create info procedure file: ${error.message}`);
    return;
  }

  try {
    await runBr(context, lexiPath, prcFileName);
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to run BR for runtime info: ${error.message}`);
    return;
  } finally {
    try {
      if (fs.existsSync(prcPath)) fs.unlinkSync(prcPath);
    } catch {
      // ignore
    }
  }

  let statusText = "";
  let configText = "";
  let cwdText = "";
  try {
    if (fs.existsSync(statusPath)) {
      statusText = fs.readFileSync(statusPath, "latin1").trim();
    }
    if (fs.existsSync(configPath)) {
      configText = fs.readFileSync(configPath, "latin1").trim();
    }
    if (fs.existsSync(cwdPath)) {
      cwdText = fs.readFileSync(cwdPath, "latin1").trim();
    }
  } catch (error: any) {
    vscode.window.showErrorMessage(`Failed to read BR info output: ${error.message}`);
    return;
  } finally {
    for (const f of [statusPath, configPath, cwdPath]) {
      try {
        if (fs.existsSync(f)) fs.unlinkSync(f);
      } catch {
        // ignore
      }
    }
  }

  // Parse version from status output (e.g. "Release 4.32g")
  let version = "(unknown)";
  const versionMatch = /Release\s+(\S+)/i.exec(statusText);
  if (versionMatch) {
    version = versionMatch[1];
  }

  // Parse OPTION 60 from config output (e.g. "OPTION 60 is OFF")
  let option60 = "(unknown)";
  const option60Match = /OPTION\s+60\s+is\s+(ON|OFF)/i.exec(configText);
  if (option60Match) {
    option60 = option60Match[1].toUpperCase();
  }

  const exeLabel = configuredExe || (process.platform === "win32"
    ? path.join(lexiPath, "brnative.exe") + " (bundled)"
    : path.join(lexiPath, "brlinux") + " (bundled)");
  const wbconfigLabel = configuredWbconfig || "(bundled default)";

  const outputChannel = vscode.window.createOutputChannel("BR Runtime Info");
  outputChannel.clear();
  outputChannel.appendLine("=== BR Runtime Info ===");
  outputChannel.appendLine(`Executable: ${exeLabel}`);
  outputChannel.appendLine(`Wbconfig: ${wbconfigLabel}`);
  outputChannel.appendLine(`BR Version: ${version}`);
  outputChannel.appendLine(`OPTION 60: ${option60}`);
  outputChannel.appendLine(`Work Directory: ${cwdText || "(unknown)"}`);
  outputChannel.show(false);
}

export function activateInfo(context: vscode.ExtensionContext) {
  context.subscriptions.push(
    vscode.commands.registerCommand("br.showInfo", () => showRuntimeInfo(context)),
  );
}
