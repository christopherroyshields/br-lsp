import { workspace, ExtensionContext, window } from "vscode";

import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
} from "vscode-languageclient/node";

let client: LanguageClient;

export async function activate(_context: ExtensionContext) {
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
    traceOutputChannel,
  };

  client = new LanguageClient(
    "br-lsp",
    "BR Language Server",
    serverOptions,
    clientOptions,
  );
  client.start();
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  return client.stop();
}
