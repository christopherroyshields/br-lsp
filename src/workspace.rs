use std::collections::HashMap;
use std::path::Path;

use tower_lsp::lsp_types::Url;

use crate::extract::FunctionDef;

#[derive(Debug, Default)]
pub struct WorkspaceIndex {
    /// Lowercase function name -> Vec<FunctionDef with uri>
    definitions: HashMap<String, Vec<IndexedFunctionDef>>,
}

#[derive(Debug, Clone)]
pub struct IndexedFunctionDef {
    pub uri: Url,
    #[allow(dead_code)]
    pub def: FunctionDef,
}

impl WorkspaceIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, uri: &Url, defs: Vec<FunctionDef>) {
        for def in defs {
            let key = def.name.to_ascii_lowercase();
            self.definitions
                .entry(key)
                .or_default()
                .push(IndexedFunctionDef {
                    uri: uri.clone(),
                    def,
                });
        }
    }

    pub fn remove_file(&mut self, uri: &Url) {
        self.definitions.retain(|_, entries| {
            entries.retain(|e| &e.uri != uri);
            !entries.is_empty()
        });
    }

    pub fn update_file(&mut self, uri: &Url, defs: Vec<FunctionDef>) {
        self.remove_file(uri);
        self.add_file(uri, defs);
    }

    pub fn lookup(&self, name: &str) -> &[IndexedFunctionDef] {
        self.definitions
            .get(&name.to_ascii_lowercase())
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Returns all definitions for `name`, sorted by priority relative to `current_uri`:
    /// 0. Local (same URI), non-import-only
    /// 1. Library, non-import-only
    /// 2. Any non-import-only
    /// 3. Import-only
    pub fn lookup_prioritized(&self, name: &str, current_uri: &str) -> Vec<&IndexedFunctionDef> {
        let mut defs: Vec<&IndexedFunctionDef> = self.lookup(name).iter().collect();
        defs.sort_by_key(|d| {
            let is_local = d.uri.as_str() == current_uri;
            match (is_local, d.def.is_import_only, d.def.is_library) {
                (true, false, _) => 0,
                (_, false, true) => 1,
                (_, false, false) => 2,
                (_, true, _) => 3,
            }
        });
        defs
    }

    /// Returns the highest-priority definition for `name` relative to `current_uri`.
    pub fn lookup_best(&self, name: &str, current_uri: &str) -> Option<&IndexedFunctionDef> {
        self.lookup_prioritized(name, current_uri).into_iter().next()
    }

    pub fn all_symbols(&self) -> Vec<&IndexedFunctionDef> {
        self.definitions.values().flatten().collect()
    }

    /// Returns one representative `IndexedFunctionDef` per unique function name,
    /// excluding entries from `exclude_uri` and import-only entries.
    /// Prefers entries with `is_library: true` when available.
    pub fn unique_functions(&self, exclude_uri: &str) -> Vec<&IndexedFunctionDef> {
        self.definitions
            .values()
            .filter_map(|entries| {
                let candidates: Vec<&IndexedFunctionDef> = entries
                    .iter()
                    .filter(|e| e.uri.as_str() != exclude_uri && !e.def.is_import_only)
                    .collect();

                if candidates.is_empty() {
                    return None;
                }

                // Prefer is_library entry, otherwise take the first
                let pick = candidates
                    .iter()
                    .find(|e| e.def.is_library)
                    .unwrap_or(&candidates[0]);
                Some(*pick)
            })
            .collect()
    }
}

/// Read a BR source file from disk, decoding from CP437 to UTF-8.
pub fn read_br_file(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    // CP437 maps to a subset of Unicode; encoding_rs doesn't have CP437 directly,
    // so we do a manual byte-to-char mapping for the 128-255 range.
    let mut output = String::with_capacity(bytes.len());
    for &b in &bytes {
        if b == 0x1A {
            continue; // Skip DOS EOF marker
        }
        output.push(cp437_to_char(b));
    }
    Ok(output)
}

/// Map a CP437 byte to its Unicode character.
fn cp437_to_char(byte: u8) -> char {
    if byte < 128 {
        byte as char
    } else {
        CP437_HIGH[byte as usize - 128]
    }
}

/// CP437 high-byte (128-255) to Unicode mapping.
const CP437_HIGH: [char; 128] = [
    '\u{00C7}', '\u{00FC}', '\u{00E9}', '\u{00E2}', '\u{00E4}', '\u{00E0}', '\u{00E5}', '\u{00E7}',
    '\u{00EA}', '\u{00EB}', '\u{00E8}', '\u{00EF}', '\u{00EE}', '\u{00EC}', '\u{00C4}', '\u{00C5}',
    '\u{00C9}', '\u{00E6}', '\u{00C6}', '\u{00F4}', '\u{00F6}', '\u{00F2}', '\u{00FB}', '\u{00F9}',
    '\u{00FF}', '\u{00D6}', '\u{00DC}', '\u{00A2}', '\u{00A3}', '\u{00A5}', '\u{20A7}', '\u{0192}',
    '\u{00E1}', '\u{00ED}', '\u{00F3}', '\u{00FA}', '\u{00F1}', '\u{00D1}', '\u{00AA}', '\u{00BA}',
    '\u{00BF}', '\u{2310}', '\u{00AC}', '\u{00BD}', '\u{00BC}', '\u{00A1}', '\u{00AB}', '\u{00BB}',
    '\u{2591}', '\u{2592}', '\u{2593}', '\u{2502}', '\u{2524}', '\u{2561}', '\u{2562}', '\u{2556}',
    '\u{2555}', '\u{2563}', '\u{2551}', '\u{2557}', '\u{255D}', '\u{255C}', '\u{255B}', '\u{2510}',
    '\u{2514}', '\u{2534}', '\u{252C}', '\u{251C}', '\u{2500}', '\u{253C}', '\u{255E}', '\u{255F}',
    '\u{255A}', '\u{2554}', '\u{2569}', '\u{2566}', '\u{2560}', '\u{2550}', '\u{256C}', '\u{2567}',
    '\u{2568}', '\u{2564}', '\u{2565}', '\u{2559}', '\u{2558}', '\u{2552}', '\u{2553}', '\u{256B}',
    '\u{256A}', '\u{2518}', '\u{250C}', '\u{2588}', '\u{2584}', '\u{258C}', '\u{2590}', '\u{2580}',
    '\u{03B1}', '\u{00DF}', '\u{0393}', '\u{03C0}', '\u{03A3}', '\u{03C3}', '\u{00B5}', '\u{03C4}',
    '\u{03A6}', '\u{0398}', '\u{03A9}', '\u{03B4}', '\u{221E}', '\u{03C6}', '\u{03B5}', '\u{2229}',
    '\u{2261}', '\u{00B1}', '\u{2265}', '\u{2264}', '\u{2320}', '\u{2321}', '\u{00F7}', '\u{2248}',
    '\u{00B0}', '\u{2219}', '\u{00B7}', '\u{221A}', '\u{207F}', '\u{00B2}', '\u{25A0}', '\u{00A0}',
];

/// Check if a file path has a BR extension (.brs or .wbs), case-insensitive.
pub fn is_br_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("brs") || e.eq_ignore_ascii_case("wbs"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{ParamInfo, ParamKind};
    use tower_lsp::lsp_types::{Position, Range};

    fn make_def(name: &str, is_library: bool) -> FunctionDef {
        FunctionDef {
            name: name.to_string(),
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 10,
                },
            },
            selection_range: Range {
                start: Position {
                    line: 0,
                    character: 4,
                },
                end: Position {
                    line: 0,
                    character: 10,
                },
            },
            is_library,
            is_import_only: false,
            params: vec![],
            has_param_substitution: false,
            documentation: None,
            return_documentation: None,
        }
    }

    fn test_url(name: &str) -> Url {
        Url::parse(&format!("file:///workspace/{name}")).unwrap()
    }

    #[test]
    fn add_and_lookup() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        index.add_file(&uri, vec![make_def("fnFoo", false)]);

        let results = index.lookup("fnFoo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].def.name, "fnFoo");
    }

    #[test]
    fn lookup_case_insensitive() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        index.add_file(&uri, vec![make_def("fnFoo", false)]);

        assert_eq!(index.lookup("FNFOO").len(), 1);
        assert_eq!(index.lookup("fnfoo").len(), 1);
    }

    #[test]
    fn remove_file() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        index.add_file(&uri, vec![make_def("fnFoo", false)]);
        index.remove_file(&uri);

        assert!(index.lookup("fnFoo").is_empty());
    }

    #[test]
    fn update_file() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        index.add_file(&uri, vec![make_def("fnFoo", false)]);
        index.update_file(&uri, vec![make_def("fnBar", true)]);

        assert!(index.lookup("fnFoo").is_empty());
        assert_eq!(index.lookup("fnBar").len(), 1);
        assert!(index.lookup("fnBar")[0].def.is_library);
    }

    #[test]
    fn multiple_files_same_function_name() {
        let mut index = WorkspaceIndex::new();
        let uri1 = test_url("a.brs");
        let uri2 = test_url("b.brs");
        index.add_file(&uri1, vec![make_def("fnFoo", false)]);
        index.add_file(&uri2, vec![make_def("fnFoo", true)]);

        assert_eq!(index.lookup("fnFoo").len(), 2);
    }

    #[test]
    fn all_symbols() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        index.add_file(
            &uri,
            vec![make_def("fnFoo", false), make_def("fnBar", true)],
        );

        assert_eq!(index.all_symbols().len(), 2);
    }

    #[test]
    fn remove_only_target_file() {
        let mut index = WorkspaceIndex::new();
        let uri1 = test_url("a.brs");
        let uri2 = test_url("b.brs");
        index.add_file(&uri1, vec![make_def("fnFoo", false)]);
        index.add_file(&uri2, vec![make_def("fnFoo", true)]);
        index.remove_file(&uri1);

        let results = index.lookup("fnFoo");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].uri, uri2);
    }

    #[test]
    fn cp437_ascii_roundtrip() {
        // ASCII bytes should pass through unchanged
        for b in 0u8..128 {
            assert_eq!(cp437_to_char(b), b as char);
        }
    }

    #[test]
    fn cp437_high_bytes() {
        // Spot check some well-known CP437 mappings
        assert_eq!(cp437_to_char(0x80), '\u{00C7}'); // Ç
        assert_eq!(cp437_to_char(0x81), '\u{00FC}'); // ü
        assert_eq!(cp437_to_char(0xE1), '\u{00DF}'); // ß
        assert_eq!(cp437_to_char(0xFE), '\u{25A0}'); // ■
    }

    #[test]
    fn is_br_file_checks() {
        assert!(is_br_file(Path::new("foo.brs")));
        assert!(is_br_file(Path::new("foo.BRS")));
        assert!(is_br_file(Path::new("foo.wbs")));
        assert!(is_br_file(Path::new("foo.WBS")));
        assert!(!is_br_file(Path::new("foo.rs")));
        assert!(!is_br_file(Path::new("foo")));
    }

    fn make_def_full(name: &str, is_library: bool, is_import_only: bool) -> FunctionDef {
        FunctionDef {
            name: name.to_string(),
            range: Range::default(),
            selection_range: Range::default(),
            is_library,
            is_import_only,
            params: vec![],
            has_param_substitution: false,
            documentation: None,
            return_documentation: None,
        }
    }

    #[test]
    fn unique_functions_dedup() {
        let mut index = WorkspaceIndex::new();
        let uri1 = test_url("a.brs");
        let uri2 = test_url("b.brs");
        index.add_file(&uri1, vec![make_def("fnFoo", false)]);
        index.add_file(&uri2, vec![make_def("fnFoo", false)]);

        let results = index.unique_functions("file:///workspace/main.brs");
        let foo_count = results.iter().filter(|r| r.def.name == "fnFoo").count();
        assert_eq!(foo_count, 1, "same-name function from 2 files should produce 1 result");
    }

    #[test]
    fn unique_functions_prefers_library() {
        let mut index = WorkspaceIndex::new();
        let uri1 = test_url("a.brs");
        let uri2 = test_url("b.brs");
        index.add_file(&uri1, vec![make_def("fnFoo", false)]);
        index.add_file(&uri2, vec![make_def("fnFoo", true)]);

        let results = index.unique_functions("file:///workspace/main.brs");
        assert_eq!(results.len(), 1);
        assert!(results[0].def.is_library, "should prefer is_library entry");
        assert_eq!(results[0].uri, uri2);
    }

    #[test]
    fn unique_functions_excludes_current_uri() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("main.brs");
        index.add_file(&uri, vec![make_def("fnLocal", false)]);

        let results = index.unique_functions("file:///workspace/main.brs");
        assert!(results.is_empty(), "should exclude entries from current URI");
    }

    #[test]
    fn unique_functions_excludes_import_only() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("lib.brs");
        index.add_file(&uri, vec![make_def_full("fnImport", false, true)]);

        let results = index.unique_functions("file:///workspace/main.brs");
        assert!(results.is_empty(), "should exclude import-only entries");
    }

    #[test]
    fn function_def_with_params_in_index() {
        let mut index = WorkspaceIndex::new();
        let uri = test_url("test.brs");
        let def = FunctionDef {
            name: "fnCalc$".to_string(),
            range: Range::default(),
            selection_range: Range::default(),
            is_library: true,
            is_import_only: false,
            params: vec![
                ParamInfo {
                    name: "X".to_string(),
                    kind: ParamKind::Numeric,
                    is_optional: false,
                    is_reference: false,
                    documentation: None,
                },
                ParamInfo {
                    name: "Y$".to_string(),
                    kind: ParamKind::String,
                    is_optional: true,
                    is_reference: true,
                    documentation: None,
                },
            ],
            has_param_substitution: false,
            documentation: None,
            return_documentation: None,
        };
        index.add_file(&uri, vec![def]);

        let results = index.lookup("fnCalc$");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].def.params.len(), 2);
        assert_eq!(results[0].def.params[1].kind, ParamKind::String);
        assert!(results[0].def.params[1].is_optional);
        assert!(results[0].def.params[1].is_reference);
    }

    #[test]
    fn lookup_prioritized_local_first() {
        let mut index = WorkspaceIndex::new();
        let local_uri = test_url("local.brs");
        let other_uri = test_url("other.brs");
        index.add_file(&local_uri, vec![make_def("fnFoo", false)]);
        index.add_file(&other_uri, vec![make_def("fnFoo", true)]);

        let results = index.lookup_prioritized("fnFoo", local_uri.as_str());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].uri, local_uri, "local def should come first");
    }

    #[test]
    fn lookup_prioritized_library_before_non_library() {
        let mut index = WorkspaceIndex::new();
        let uri_a = test_url("a.brs");
        let uri_b = test_url("b.brs");
        index.add_file(&uri_a, vec![make_def("fnFoo", false)]);
        index.add_file(&uri_b, vec![make_def("fnFoo", true)]);

        // Neither is local
        let results = index.lookup_prioritized("fnFoo", "file:///workspace/other.brs");
        assert_eq!(results.len(), 2);
        assert!(results[0].def.is_library, "library def should come before non-library");
    }

    #[test]
    fn lookup_prioritized_import_only_last() {
        let mut index = WorkspaceIndex::new();
        let uri_a = test_url("a.brs");
        let uri_b = test_url("b.brs");
        index.add_file(&uri_a, vec![make_def_full("fnFoo", false, true)]);
        index.add_file(&uri_b, vec![make_def("fnFoo", false)]);

        let results = index.lookup_prioritized("fnFoo", "file:///workspace/other.brs");
        assert_eq!(results.len(), 2);
        assert!(!results[0].def.is_import_only, "non-import should come first");
        assert!(results[1].def.is_import_only, "import-only should come last");
    }

    #[test]
    fn lookup_best_returns_local() {
        let mut index = WorkspaceIndex::new();
        let local_uri = test_url("local.brs");
        let other_uri = test_url("other.brs");
        index.add_file(&other_uri, vec![make_def("fnFoo", true)]);
        index.add_file(&local_uri, vec![make_def("fnFoo", false)]);

        let best = index.lookup_best("fnFoo", local_uri.as_str()).unwrap();
        assert_eq!(best.uri, local_uri, "lookup_best should return local def");
    }

    #[test]
    fn lookup_best_empty() {
        let index = WorkspaceIndex::new();
        assert!(index.lookup_best("fnNonexistent", "file:///x.brs").is_none());
    }
}
