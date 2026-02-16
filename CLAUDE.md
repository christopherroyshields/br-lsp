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

### Rust LSP Server (`src/main.rs`)
Single-binary server using `tower-lsp`. The `Backend` struct implements the `LanguageServer` trait and holds:
- `client: Client` — tower-lsp client handle for sending notifications (diagnostics, etc.) back to the editor
- `document_map: DashMap<String, Rope>` — concurrent map of open document URIs to their content (using `ropey` for efficient text manipulation)

The server communicates over stdin/stdout. Document sync uses `TextDocumentSyncKind::FULL` (entire document content sent on each change). The `on_change` method is the central hook called on document open/edit — this is where parsing, diagnostics, and analysis should be wired in.

### VS Code Extension Client (`client/src/extension.ts`)
Thin client that spawns the `br-lsp` binary and connects via `vscode-languageclient`. Watches `**/*.{brs,wbs}` files. The server binary path can be overridden via the `SERVER_PATH` environment variable.

### Build Pipeline
The extension is bundled with esbuild (`esbuild.js`), outputting to `dist/extension.js`. The entry point is `client/src/extension.ts`.

## Toolchain
- Rust 1.88.0 (pinned in `rust-toolchain.toml`)
- pnpm for JS package management
- CI runs both Rust (fmt, clippy, build) and JS (fmt, lint) checks
