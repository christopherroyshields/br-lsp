import { commands, Uri, workspace, ExtensionContext, window } from "vscode";

import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

export async function activate(context: ExtensionContext) {
  const outputChannel = window.createOutputChannel("BR Language Server");
  outputChannel.appendLine("Starting BR Language Server...");
  const traceOutputChannel = window.createOutputChannel("BR Language Server trace");
  const command = process.env.SERVER_PATH || "br-lsp";
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
    ],
    synchronize: {
      fileEvents: workspace.createFileSystemWatcher("**/*.{brs,wbs}"),
    },
    outputChannel,
    traceOutputChannel,
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
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
