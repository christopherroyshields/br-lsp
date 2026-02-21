import {
  commands,
  languages,
  window,
  Disposable,
  ExtensionContext,
  Hover,
  Range,
  Position,
  MarkdownString,
  StatusBarItem,
  StatusBarAlignment,
} from "vscode";
import type { TextDocument, HoverProvider, CancellationToken } from "vscode";
import { LanguageClient } from "vscode-languageclient/node";

interface InspectNodeResult {
  kind: string;
  isNamed: boolean;
  text: string;
  range: { start: { line: number; character: number }; end: { line: number; character: number } };
  ancestors: string[];
  childCount: number;
  namedChildCount: number;
  isError: boolean;
  isMissing: boolean;
  hasError: boolean;
  namedAncestor: { kind: string; range: { start: { line: number; character: number }; end: { line: number; character: number } } } | null;
  semanticToken: { type: string; modifiers: string[] } | null;
}

let active = false;
let hoverRegistration: Disposable | undefined;
let statusBarItem: StatusBarItem | undefined;

export function activateInspector(context: ExtensionContext, client: LanguageClient) {
  context.subscriptions.push(
    commands.registerCommand("br-lsp.toggleInspector", () => {
      if (active) {
        disable();
      } else {
        enable(client);
      }
    }),
  );
}

function enable(client: LanguageClient) {
  active = true;

  const provider: HoverProvider = {
    async provideHover(document: TextDocument, position: Position, _token: CancellationToken): Promise<Hover | null> {
      let result: InspectNodeResult | null;
      try {
        result = await client.sendRequest<InspectNodeResult | null>("workspace/executeCommand", {
          command: "br-lsp.inspectNode",
          arguments: [document.uri.toString(), position.line, position.character],
        });
      } catch {
        return null;
      }

      if (!result) return null;

      const md = buildMarkdown(result);
      const r = result.range;
      const range = new Range(
        new Position(r.start.line, r.start.character),
        new Position(r.end.line, r.end.character),
      );
      return new Hover(md, range);
    },
  };

  // Register with high priority (first in chain) so it shows alongside normal hovers
  hoverRegistration = languages.registerHoverProvider(
    [{ language: "br" }, { language: "lay" }],
    provider,
  );

  statusBarItem = window.createStatusBarItem(StatusBarAlignment.Right, 100);
  statusBarItem.command = "br-lsp.toggleInspector";
  statusBarItem.text = "$(telescope) Inspector ON";
  statusBarItem.tooltip = "Tree-sitter Inspector (click to toggle)";
  statusBarItem.show();
}

function disable() {
  active = false;

  hoverRegistration?.dispose();
  hoverRegistration = undefined;

  statusBarItem?.hide();
  statusBarItem?.dispose();
  statusBarItem = undefined;
}

function buildMarkdown(result: InspectNodeResult): MarkdownString {
  const md = new MarkdownString();
  md.isTrusted = true;

  const namedLabel = result.isNamed ? "*(named)*" : "*(anonymous)*";
  md.appendMarkdown(`**\`${result.kind}\`** ${namedLabel}\n\n`);
  md.appendMarkdown(`Text: \`${result.text}\`\n\n`);

  if (result.ancestors.length > 0) {
    const path = result.ancestors.map((a) => `\`${a}\``).join(" > ");
    md.appendMarkdown(`Path: ${path}\n\n`);
  }

  md.appendMarkdown(`Children: ${result.childCount} (${result.namedChildCount} named)\n\n`);

  const r = result.range;
  md.appendMarkdown(`Range: [${r.start.line}:${r.start.character}]-[${r.end.line}:${r.end.character}]\n\n`);

  if (result.semanticToken) {
    const mods = result.semanticToken.modifiers.length > 0 ? ` [${result.semanticToken.modifiers.join(", ")}]` : "";
    md.appendMarkdown(`Semantic: **${result.semanticToken.type}**${mods}\n\n`);
  }

  if (result.namedAncestor) {
    const na = result.namedAncestor;
    md.appendMarkdown(`Named ancestor: \`${na.kind}\` [${na.range.start.line}:${na.range.start.character}]-[${na.range.end.line}:${na.range.end.character}]\n\n`);
  }

  const flags: string[] = [];
  if (result.isError) flags.push("ERROR");
  if (result.isMissing) flags.push("MISSING");
  if (result.hasError) flags.push("has error");
  if (flags.length > 0) {
    md.appendMarkdown(`Flags: ${flags.join(", ")}\n\n`);
  }

  return md;
}
