# Changelog

All notable changes to the BR Language Extension are documented in this file.

## [0.1.22] — 2026-07-31

### Changed

- Updated extension icon

## [0.1.21] — 2026-07-31

First stable release since 0.1.17, rolling up the 0.1.18–0.1.20 pre-releases below.

### Fixed

- Readme documented launch configuration behavior that no longer matched 0.1.20; corrected

## [0.1.20] — 2026-07-25

### Added

- Selectable BR runtime version. **BR: Select Runtime Version** (or the new status bar item) switches between the bundled BR 4.3 and BR 4.2 runtimes, stored in the `br.runtimeVersion` setting. The selection applies to everything that spawns BR — compile, run, decompile, add/strip line numbers, proc search, sourcemap generation, and Show Runtime Info. This restores the capability vslang-br had as "Lexi: Set BR Version", without copying files over the extension's install directory
- `Lexi/brnative.42.exe` — the BR 4.2 Windows runtime is bundled again. BR 4.2 is Windows-only; the bundled Linux runtime remains 4.3

### Changed

- **BR: Show Runtime Info** now reports the executable actually resolved by the compile pipeline and the selected runtime, instead of recomputing a label that could disagree with what ran
- `br` launch configurations no longer default `executable` to a hard-coded `brnative.exe` path — omit it to follow `br.runtimeVersion`. An explicit `executable` still overrides, for **BR: Run** only
- `br.executable` continues to override everything; the status bar shows `BR (custom)` when it is set

### Removed

- `Lexi/brnative.43` — a byte-identical duplicate of the bundled 4.3 runtime that nothing referenced (−2.7 MB)

### Note

Compiled `.br`/`.wb` objects are version-specific. After switching runtime versions, recompile before running or decompiling.

## [0.1.19] — 2026-07-25

### Fixed

- `OPTION 60` removed from the bundled `wbconfig.sys`. It is a BR 4.1-and-earlier compatibility switch and was enabled by mistake in 0.1.16, when the old per-version wbconfig profiles (`wbconfig.s41`/`wbconfig.s42`) were merged into a single file — only the 4.1 profile had ever set it. With it on, programs using 4.2/4.3-only features failed to compile

### Changed

- **BR: Show Runtime Info** now explains what `OPTION 60: ON` means, for anyone whose `br.wbconfig` points at a copy of the old bundled file

## [0.1.18] — 2026-07-24

### Fixed

- False "source file may be out of date" warning: the compiled file was flagged as newer than its source on a sub-second timestamp difference, which happens routinely because compiling writes the `.br` right after the `.brs` is saved. The check now allows a one-minute tolerance ([#4](https://github.com/christopherroyshields/br-lsp/issues/4))

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
