import * as vscode from "vscode";
import * as path from "path";
import * as fs from "fs";

export type BrRuntimeVersion = "4.2" | "4.3";

export const DEFAULT_RUNTIME_VERSION: BrRuntimeVersion = "4.3";

export function getLexiPath(context: vscode.ExtensionContext): string {
  return path.join(context.extensionPath, "Lexi");
}

export function normalizeRuntimeVersion(value: string | undefined): BrRuntimeVersion {
  return value === "4.2" || value === "4.3" ? value : DEFAULT_RUNTIME_VERSION;
}

/**
 * Filename of the bundled runtime for a platform/version pair, or undefined when
 * no such runtime ships (BR 4.2 is Windows-only).
 */
export function bundledRuntimeFile(
  version: BrRuntimeVersion,
  platform: NodeJS.Platform,
): string | undefined {
  if (platform === "win32") {
    return version === "4.2" ? "brnative.42.exe" : "brnative.exe";
  }
  return version === "4.2" ? undefined : "brlinux";
}

export interface ResolvedRuntime {
  /** Absolute path to the executable. Empty only when no runtime could be named. */
  path: string;
  /** The requested version. Meaningful only when `bundled` is true. */
  version: BrRuntimeVersion;
  /** False when `br.executable` overrides the bundled runtime — version is then unknown. */
  bundled: boolean;
  /** Set when the request cannot be satisfied; callers must not spawn. */
  error?: string;
}

export interface ResolveOptions {
  lexiPath: string;
  version: BrRuntimeVersion;
  platform: NodeJS.Platform;
  configuredExe: string;
  /** Injectable for tests; defaults to fs.existsSync. */
  exists?: (filePath: string) => boolean;
}

/**
 * Single source of truth for which BR executable runs. `br.executable` wins over
 * `br.runtimeVersion`; otherwise the bundled runtime for the selected version is used.
 */
export function resolveBrExecutable(opts: ResolveOptions): ResolvedRuntime {
  const version = normalizeRuntimeVersion(opts.version);
  const exists = opts.exists ?? fs.existsSync;

  if (opts.configuredExe) {
    const configured = opts.configuredExe;
    return exists(configured)
      ? { path: configured, version, bundled: false }
      : {
          path: configured,
          version,
          bundled: false,
          error: `BR executable not found: ${configured}`,
        };
  }

  const file = bundledRuntimeFile(version, opts.platform);
  if (!file) {
    return {
      path: "",
      version,
      bundled: true,
      error:
        'BR 4.2 is bundled for Windows only. Set "br.runtimeVersion" to "4.3", or point "br.executable" at a 4.2 runtime.',
    };
  }

  const resolved = path.join(opts.lexiPath, file);
  if (!exists(resolved)) {
    return {
      path: resolved,
      version,
      bundled: true,
      error: `Bundled BR ${version} runtime not found: ${resolved}`,
    };
  }

  return { path: resolved, version, bundled: true };
}

/** Short label for UI. A user-supplied executable has no version we can claim. */
export function runtimeLabel(runtime: ResolvedRuntime): string {
  return runtime.bundled ? `BR ${runtime.version}` : "BR (custom)";
}

export function getRuntimeVersion(): BrRuntimeVersion {
  const config = vscode.workspace.getConfiguration("br");
  return normalizeRuntimeVersion(config.get<string>("runtimeVersion", DEFAULT_RUNTIME_VERSION));
}

/** Binds the pure resolver to the current settings and platform. */
export function resolveRuntime(lexiPath: string): ResolvedRuntime {
  const config = vscode.workspace.getConfiguration("br");
  return resolveBrExecutable({
    lexiPath,
    version: getRuntimeVersion(),
    platform: process.platform,
    configuredExe: config.get<string>("executable", ""),
  });
}

let statusBarItem: vscode.StatusBarItem | undefined;

function updateStatusBar(context: vscode.ExtensionContext): void {
  if (!statusBarItem) {
    return;
  }
  const runtime = resolveRuntime(getLexiPath(context));
  const label = runtimeLabel(runtime);

  statusBarItem.text = runtime.error ? `$(warning) ${label}` : `$(server) ${label}`;
  statusBarItem.backgroundColor = runtime.error
    ? new vscode.ThemeColor("statusBarItem.warningBackground")
    : undefined;

  const detail =
    runtime.error ??
    (runtime.bundled
      ? runtime.path
      : `${runtime.path} (set by br.executable — br.runtimeVersion does not apply)`);
  statusBarItem.tooltip = `BR runtime: ${detail}\nClick to change`;
  statusBarItem.show();
}

interface VersionPick extends vscode.QuickPickItem {
  version: BrRuntimeVersion;
}

async function selectRuntimeVersion(): Promise<void> {
  const current = getRuntimeVersion();
  const windows = process.platform === "win32";

  const items: VersionPick[] = [
    {
      label: "BR 4.3",
      description: current === "4.3" ? "bundled • current" : "bundled",
      version: "4.3",
    },
    {
      label: "BR 4.2",
      description: windows
        ? current === "4.2"
          ? "bundled • current"
          : "bundled"
        : "Windows only — not bundled for Linux",
      version: "4.2",
    },
  ];

  const picked = await vscode.window.showQuickPick(items, {
    placeHolder: "Select the BR runtime used to compile and run programs",
  });
  if (!picked || picked.version === current) {
    return;
  }

  if (picked.version === "4.2" && !windows) {
    vscode.window.showWarningMessage(
      "BR 4.2 is bundled for Windows only. Set br.executable to use a 4.2 runtime on this platform.",
    );
    return;
  }

  const config = vscode.workspace.getConfiguration("br");
  const inspected = config.inspect<string>("runtimeVersion");
  // Write back where the value already lives; fall back to the workspace when one is open.
  let target: vscode.ConfigurationTarget;
  if (inspected?.workspaceValue !== undefined) {
    target = vscode.ConfigurationTarget.Workspace;
  } else if (inspected?.globalValue !== undefined) {
    target = vscode.ConfigurationTarget.Global;
  } else if (vscode.workspace.workspaceFolders?.length) {
    target = vscode.ConfigurationTarget.Workspace;
  } else {
    target = vscode.ConfigurationTarget.Global;
  }

  await config.update("runtimeVersion", picked.version, target);

  const where = target === vscode.ConfigurationTarget.Workspace ? "workspace" : "user";
  vscode.window.showInformationMessage(
    `BR runtime set to ${picked.version} (${where} settings). Compiled programs are version-specific — recompile before running.`,
  );
}

export function activateRuntime(context: vscode.ExtensionContext) {
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 99);
  statusBarItem.name = "BR Runtime Version";
  statusBarItem.command = "br.selectRuntimeVersion";
  updateStatusBar(context);

  context.subscriptions.push(
    statusBarItem,
    vscode.commands.registerCommand("br.selectRuntimeVersion", () => selectRuntimeVersion()),
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (
        event.affectsConfiguration("br.runtimeVersion") ||
        event.affectsConfiguration("br.executable")
      ) {
        updateStatusBar(context);
      }
    }),
  );
}
