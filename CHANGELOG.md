# Changelog

All notable changes to the BR Language Extension are documented in this file.

## [0.1.17] — 2026-07-22

### Changed

- New extension icon: retro terminal design rendered from `logo.svg`, replacing the old red logo

## [0.1.16] — 2026-07-06

### Added

- Sourcemap generation during compilation: Lexi emits a `.map` file (BR line ↔ source line) alongside compiled output, opt-in via the **BR: Generate Source Map** command
- **BR: Show Runtime Info** command displaying BR version, serial number, and work directory
- `br.executable` and `br.wbconfig` settings to configure the BR executable and wbconfig used for compile/run
- Unit tests for decompile and file searching

### Changed

- Command prefix renamed from `br-lsp` to `br`
- Bundled Lexi runtime updated to BR 4.3 (420o); `OPTION 60` enabled in wbconfig
- Compiled-file handler (`BRCompiledFS`) reads from the `.brs` on disk and saves to both `.brs` and `.br`
- Document symbols are computed with a cached TreeCursor walk for better performance on large files
- BR error messages now include the proc filename for easier debugging

### Fixed

- Short-form line numbers (e.g. `10 print a`) are now detected by the compile pipeline; previously only zero-padded forms (`00010 print a`) were recognized, causing already-numbered source to be re-numbered ([#3](https://github.com/christopherroyshields/br-lsp/issues/3))

## [0.1.15] — 2026-02-28

First release of the rewritten extension (successor to vslang-br 0.0.x), built on a Rust language server using tree-sitter.

### Added

- Language server: syntax diagnostics, find references, go to definition, rename, completions, document symbols, and semantic highlighting — scope-aware for functions, labels, line numbers, and variables
- **BR: Compile Program** with bundled BR runtime, auto-compile on save (per-file toggle), and compile error reporting
- **BR: Run Program** with launch configurations and dynamic wbconfig
- **BR: Decompile Program** for single files and whole folders, with a custom editor that intercepts compiled BR file opens
- Proc Search: search inside compiled BR programs from the activity bar
- Lexi integration: add/strip line numbers commands
- Auto line numbering on Enter, next/previous occurrence commands, comment toggling, bracket matching, and indentation rules
- Layout file (`.lay`) support and library-aware function resolution
- Configurable diagnostics by category
- File icons for BR source and compiled files
