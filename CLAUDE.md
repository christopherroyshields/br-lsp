# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

br-lsp is a Language Server Protocol (LSP) implementation for the Business Rules (BR) programming language. It consists of two parts: a Rust-based LSP server built on `tower-lsp`, and a TypeScript VS Code extension client.

BR source files use `.brs` and `.wbs` extensions and are encoded with CP437.

## Build Commands

### Rust (LSP server)
- `cargo build` — build the server binary
- `cargo lint` — run clippy with warnings-as-errors (alias defined in `.cargo/config.toml`)
- `cargo fmt --check` — check formatting
- `cargo test` — run tests

### TypeScript (VS Code extension client)
- `pnpm i` — install all dependencies (also runs `postinstall` for client/)
- `pnpm run compile` — type-check and bundle the extension
- `pnpm run lint` — lint with oxlint
- `pnpm run fmt:check` — check formatting with oxfmt

### VS Code development
1. `pnpm i && cargo build`
2. Press F5 in VS Code to launch the extension development host
3. The debug launch config sets `SERVER_PATH` to `target/debug/br-lsp`

### Package the extension
- `pnpm run package` — creates a `.vsix` file

## Architecture

### Rust LSP Server
Single-binary server using `tower-lsp`, organized across four modules:

- **`src/main.rs`** — entry point, wires up `Backend` with `LspService` over stdin/stdout
- **`src/backend.rs`** — `Backend` struct implementing `LanguageServer` trait, `DocumentState`, and all LSP method handlers
- **`src/parser.rs`** — tree-sitter integration: parser creation, parsing, diagnostics collection, and query helpers (`run_query`, `node_at_position`)
- **`src/references.rs`** — "find all references" logic with scope-aware variable resolution (function parameters vs. module-level)

The `Backend` struct holds:
- `client: Client` — tower-lsp client handle for sending notifications back to the editor
- `document_map: DashMap<String, DocumentState>` — concurrent map of open document URIs to their state
- `parser: Mutex<Parser>` — shared tree-sitter parser (Parser is `Send` but not `Sync`)

`DocumentState` stores per-document data:
- `rope: Rope` — efficient text representation
- `source: String` — raw source text (used for tree-sitter queries)
- `tree: Option<Tree>` — parsed tree-sitter AST

The `on_change` method reparses the full document on each edit (`TextDocumentSyncKind::FULL`), collects diagnostics from tree-sitter error/missing nodes, and publishes them.

**Currently implemented LSP capabilities:**
- Text document sync (full)
- Diagnostics (syntax errors from tree-sitter)
- Find references (functions, labels, line numbers, variables — scope-aware)
- Completion provider (registered but returns `None`)

### VS Code Extension Client (`client/src/extension.ts`)
Thin client that spawns the `br-lsp` binary and connects via `vscode-languageclient`. Watches `**/*.{brs,wbs}` files. The server binary path can be overridden via the `SERVER_PATH` environment variable.

### Build Pipeline
The extension is bundled with esbuild (`esbuild.js`), outputting to `dist/extension.js`. The entry point is `client/src/extension.ts`.

## Toolchain
- Rust 1.88.0 (pinned in `rust-toolchain.toml`)
- pnpm for JS package management
- CI runs both Rust (fmt, clippy, build) and JS (fmt, lint) checks
