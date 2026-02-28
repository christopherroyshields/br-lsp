# BR Language Server

VS Code extension providing language intelligence, compile/run, and decompile support for the Business Rules (BR) programming language.

## Features

### Language Intelligence

- **Diagnostics** — syntax errors, function issues, undefined functions, unused variables
- **Completions** with resolve support
- **Hover** — builtin and user-defined function signatures
- **Signature help** — parameter hints for builtin and user-defined functions as you type
- **Go to definition**
- **Find references** — scope-aware variable resolution
- **Rename** — workspace-wide, scope-aware
- **Document & workspace symbols**
- **Code actions** — quick fixes
- **Semantic token highlighting**

### Compile & Run

- Compile `.brs`/`.wbs` to `.br`/`.wb` via the Lexi preprocessor (`Ctrl+Shift+B`)
- Auto-compile on save with status bar toggle
- Run with integrated terminal (`Ctrl+Shift+R`)
- Launch configurations via `.vscode/launch.json`
- Cross-platform: Windows (`brnative.exe`) and Linux (`brlinux`)

### Decompile

- Decompile `.br`/`.bro`/`.wb`/`.wbo` back to source
- Batch decompile entire folders
- Auto-decompile when opening compiled files
- Configurable style formatting
- Explorer context menu integration

### Editor Enhancements

- Auto line numbering on Enter (configurable increment and padding)
- Next/previous occurrence navigation (`Ctrl+Shift+Down`/`Up`)
- Tree-sitter node inspector (`Ctrl+Shift+Alt+I`)
- Workspace-wide diagnostic scan with CSV export (`Ctrl+Alt+7`)
- Code snippets for file I/O, Lexi, loops, and statements

## Getting Started

### Requirements

- VS Code 1.66+
- A BR runtime (`brnative.exe` or `brlinux`) for compile/run features

### Development

1. `pnpm i && cargo build`
2. Press F5 in VS Code to launch the Extension Development Host
3. Open a `.brs` or `.wbs` file

## Settings

| Setting | Type | Default | Description |
|---------|------|---------|-------------|
| `br.autoLineNumbers.enabled` | `boolean` | `true` | Auto-add line numbers on Enter |
| `br.autoLineNumbers.increment` | `number` | `10` | Default line number increment |
| `br.autoLineNumbers.zeroPadding` | `number` | `5` | Digits for line numbers (e.g. 5 → `00100`) |
| `br.diagnostics.syntax` | `boolean` | `true` | Report syntax errors |
| `br.diagnostics.functions` | `boolean` | `true` | Report function diagnostics |
| `br.diagnostics.undefinedFunctions` | `boolean` | `true` | Report undefined function calls |
| `br.diagnostics.unusedVariables` | `boolean` | `true` | Report unused DIM variables and LIBRARY imports |
| `br.decompile.sourceExtensions` | `object` | `{".br":".brs", ...}` | Compiled → source extension mapping |
| `br.decompile.styleCommand` | `string` | `"indent 2 45 keywords lower..."` | Style command applied after decompiling |
| `br.trace.server` | `string` | `"off"` | Trace communication with the language server |

## Launch Configuration

Add a `br` configuration to `.vscode/launch.json`:

```json
{
  "version": "0.2.0",
  "configurations": [
    {
      "type": "br",
      "request": "launch",
      "name": "Run BR",
      "executable": "${extensionPath}/Lexi/brnative.exe",
      "wbconfig": "",
      "wsid": "",
      "cwd": "${fileDirname}"
    }
  ]
}
```

| Property | Description |
|----------|-------------|
| `executable` | Path to BR executable. Defaults to bundled `brnative.exe`. |
| `wbconfig` | Path to wbconfig file (passed as `-[filename]`). |
| `wsid` | Workstation ID (e.g. `42`, `42+`, `21+5`, `WSIDCLEAR`). |
| `cwd` | Working directory. Defaults to `${fileDirname}`. |

All properties support VS Code variables: `${workspaceFolder}`, `${extensionPath}`, `${file}`, `${fileBasename}`, `${fileDirname}`.

## Keybindings

| Keybinding | Command | Context |
|------------|---------|---------|
| `Ctrl+Alt+7` | Scan All Project Source | Global |
| `Ctrl+Shift+Down` | Next Occurrence | BR editor |
| `Ctrl+Shift+Up` | Previous Occurrence | BR editor |
| `Ctrl+Shift+B` | Compile Program | BR editor |
| `Ctrl+Shift+R` | Run Program | BR editor |
| `Enter` | Auto Insert Line Number | BR editor (no widget open) |
| `Ctrl+Shift+Alt+I` | Toggle Tree-Sitter Inspector | BR editor |

## License

MIT
