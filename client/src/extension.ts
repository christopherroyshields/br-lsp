import * as fs from "fs";
import * as path from "path";
import { commands, Uri, workspace, ExtensionContext, window } from "vscode";

import { activateCompile } from "./compile";
import { activateDecompile } from "./decompile";
import { activateInspector } from "./inspector";
import { activateLexi } from "./lexi";
import { activateLineNumbers } from "./line-numbers";
import { activateNextPrev } from "./next-prev";
import { activateRun } from "./run";
import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  CloseHandlerResult,
  ErrorHandlerResult,
  ErrorAction,
  CloseAction,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

export async function activate(context: ExtensionContext) {
  const outputChannel = window.createOutputChannel("BR Language Server");
  outputChannel.appendLine("Starting BR Language Server...");
  const traceOutputChannel = window.createOutputChannel("BR Language Server trace");
  const binaryName = process.platform === "win32" ? "br-lsp.exe" : "br-lsp-linux";
  const bundledPath = path.join(context.extensionPath, "server", binaryName);
  if (process.platform !== "win32") {
    try {
      fs.chmodSync(bundledPath, 0o755);
    } catch {}
  }
  const command = process.env.SERVER_PATH || bundledPath;
  const run: Executable = {
    command,
    options: {
      env: {
        ...process.env,
        // eslint-disable-next-line @typescript-eslint/naming-convention
        RUST_LOG: "debug",
      },
    },
  };
  const serverOptions: ServerOptions = {
    run,
    debug: run,
  };
  let clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "br" },
      { scheme: "br-compiled", language: "br" },
      { scheme: "file", language: "lay" },
    ],
    synchronize: {
      fileEvents: [
        workspace.createFileSystemWatcher("**/*.{brs,wbs}"),
        workspace.createFileSystemWatcher("**/*.lay"),
        workspace.createFileSystemWatcher("**/filelay/*"),
      ],
    },
    outputChannel,
    traceOutputChannel,
    errorHandler: {
      error(_error: Error, _message: undefined, count: number): ErrorHandlerResult {
        if (count <= 3) {
          return { action: ErrorAction.Continue };
        }
        return { action: ErrorAction.Shutdown };
      },
      closed(): CloseHandlerResult {
        return { action: CloseAction.Restart, message: "BR Language Server crashed. Restarting..." };
      },
    },
  };

  client = new LanguageClient(
    "br-lsp",
    "BR Language Server",
    serverOptions,
    clientOptions,
  );
  await client.start();

  const scanAllCmd = commands.registerCommand("br-lsp.scanAll", async () => {
    const result = await client.sendRequest<{ summary: string; csv: string }>("workspace/executeCommand", {
      command: "br-lsp.scanAll",
      arguments: [],
    });

    if (!result) {
      window.showInformationMessage("No results from scan.");
      return;
    }

    const action = await window.showInformationMessage(result.summary, "Export CSV");

    if (action === "Export CSV" && result.csv) {
      const uri = await window.showSaveDialog({
        defaultUri: Uri.joinPath(workspace.workspaceFolders?.[0]?.uri ?? Uri.file(""), "br-diagnostics.csv"),
        filters: { "CSV files": ["csv"] },
      });

      if (uri) {
        await workspace.fs.writeFile(uri, new Uint8Array(Buffer.from(result.csv, "utf-8")));
        window.showInformationMessage(`Diagnostics exported to ${uri.fsPath}`);
      }
    }
  });
  context.subscriptions.push(scanAllCmd);

  activateCompile(context);
  activateDecompile(context);
  activateInspector(context, client);
  activateRun(context);
  activateLexi(context);
  activateLineNumbers(context);
  activateNextPrev(context);
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
