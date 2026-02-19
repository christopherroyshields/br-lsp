use std::collections::HashMap;
use std::path::Path;

use tower_lsp::lsp_types::SemanticToken;

use crate::semantic_tokens::{encode_deltas, RawToken};

// Token type indices (from TOKEN_TYPES in semantic_tokens.rs)
const TT_VARIABLE: u32 = 1;
const TT_KEYWORD: u32 = 3;
const TT_COMMENT: u32 = 4;
const TT_STRING: u32 = 5;
const TT_NUMBER: u32 = 6;
const TT_INVALID: u32 = 11;

// ---------------------------------------------------------------------------
// Valid form specs (case-insensitive)
// ---------------------------------------------------------------------------

const VALID_FORMS: &[&str] = &[
    "BH", "BL", "B", "CC", "CR", "C", "DH", "DL", "DT", "D", "GF", "GZ", "G", "L", "NZ", "N",
    "PIC", "PD", "P", "SKIP", "S", "V", "X", "ZD",
];

fn is_valid_form(spec: &str) -> bool {
    let upper = spec.to_ascii_uppercase();
    VALID_FORMS.iter().any(|f| *f == upper)
}

// ---------------------------------------------------------------------------
// Layout data structures
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct LayoutSubscript {
    pub name: String,
    pub description: String,
    pub format: String,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct LayoutKey {
    pub path: String,
    pub key_fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub path: String,
    pub prefix: String,
    #[allow(dead_code)]
    pub version: Option<u32>,
    #[allow(dead_code)]
    pub keys: Vec<LayoutKey>,
    pub subscripts: Vec<LayoutSubscript>,
    #[allow(dead_code)]
    pub record_length: Option<u32>,
}

// ---------------------------------------------------------------------------
// LayoutIndex
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct LayoutIndex {
    layouts: HashMap<String, Layout>,
}

impl LayoutIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, uri: &str, layout: Layout) {
        self.layouts.insert(uri.to_string(), layout);
    }

    pub fn remove(&mut self, uri: &str) {
        self.layouts.remove(uri);
    }

    pub fn update(&mut self, uri: &str, layout: Layout) {
        self.layouts.insert(uri.to_string(), layout);
    }

    pub fn all_layouts(&self) -> impl Iterator<Item = &Layout> {
        self.layouts.values()
    }
}

// ---------------------------------------------------------------------------
// State machine for parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Initial,
    Header,
    Fields,
    Eof,
}

// ---------------------------------------------------------------------------
// Layout parser
// ---------------------------------------------------------------------------

pub fn parse(source: &str) -> Option<Layout> {
    let mut state = State::Initial;
    let mut path = String::new();
    let mut prefix = String::new();
    let mut version: Option<u32> = None;
    let mut keys = Vec::new();
    let mut subscripts = Vec::new();
    let mut record_length: Option<u32> = None;

    for line in source.lines() {
        let trimmed = line.trim();

        if state == State::Eof {
            break;
        }

        // Comment lines
        if trimmed.starts_with('!') {
            continue;
        }

        // #eof# marker
        if trimmed.eq_ignore_ascii_case("#eof#") {
            state = State::Eof;
            continue;
        }

        // Empty lines
        if trimmed.is_empty() {
            continue;
        }

        match state {
            State::Initial => {
                // First non-empty, non-comment line is the header: path, prefix, version
                let parts: Vec<&str> = trimmed.splitn(3, ',').collect();
                path = parts
                    .first()
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                prefix = parts
                    .get(1)
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                version = parts.get(2).and_then(|s| s.trim().parse().ok());
                state = State::Header;
            }
            State::Header => {
                // Could be a key line, recl line, separator, or transition to fields
                if is_separator(trimmed) {
                    state = State::Fields;
                } else if trimmed.to_ascii_lowercase().starts_with("recl") {
                    // recl=N
                    if let Some(val) = parse_recl_value(trimmed) {
                        record_length = Some(val);
                    }
                } else {
                    // Key line: path, field1, field2, ...
                    let parts: Vec<&str> = trimmed.split(',').collect();
                    if !parts.is_empty() {
                        let key_path = parts[0].trim().to_string();
                        let key_fields: Vec<String> = parts[1..]
                            .iter()
                            .map(|s| s.trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                        keys.push(LayoutKey {
                            path: key_path,
                            key_fields,
                        });
                    }
                }
            }
            State::Fields => {
                // Field lines: name, description, spec+length [, comment]
                let parts: Vec<&str> = trimmed.splitn(4, ',').collect();
                if parts.len() >= 3 {
                    let name = parts[0].trim().to_string();
                    let description = parts
                        .get(1)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    let format = parts
                        .get(2)
                        .map(|s| s.trim().to_string())
                        .unwrap_or_default();
                    subscripts.push(LayoutSubscript {
                        name,
                        description,
                        format,
                    });
                }
            }
            State::Eof => break,
        }
    }

    if path.is_empty() {
        return None;
    }

    Some(Layout {
        path,
        prefix,
        version,
        keys,
        subscripts,
        record_length,
    })
}

fn is_separator(line: &str) -> bool {
    !line.is_empty() && line.chars().all(|c| c == '-' || c == '=')
}

fn parse_recl_value(line: &str) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    let after = lower.strip_prefix("recl")?;
    let after = after.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
    after.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Semantic tokens for layout files
// ---------------------------------------------------------------------------

pub fn collect_layout_tokens(source: &str) -> Vec<SemanticToken> {
    let mut raw = Vec::new();
    let mut state = State::Initial;

    for (line_idx, line) in source.lines().enumerate() {
        let line_num = line_idx as u32;
        let trimmed = line.trim();

        if state == State::Eof {
            // Post-eof: everything is comment
            if !line.is_empty() {
                raw.push(RawToken {
                    line: line_num,
                    start: 0,
                    length: line.len() as u32,
                    token_type: TT_COMMENT,
                    modifiers: 0,
                });
            }
            continue;
        }

        // Comment lines
        if trimmed.starts_with('!') {
            let offset = leading_spaces(line) as u32;
            raw.push(RawToken {
                line: line_num,
                start: offset,
                length: (line.len() - offset as usize) as u32,
                token_type: TT_COMMENT,
                modifiers: 0,
            });
            continue;
        }

        // #eof# marker
        if trimmed.eq_ignore_ascii_case("#eof#") {
            let offset = leading_spaces(line) as u32;
            raw.push(RawToken {
                line: line_num,
                start: offset,
                length: trimmed.len() as u32,
                token_type: TT_COMMENT,
                modifiers: 0,
            });
            state = State::Eof;
            continue;
        }

        // Empty lines — skip
        if trimmed.is_empty() {
            continue;
        }

        match state {
            State::Initial => {
                // Header line: path, prefix, version
                tokenize_header_line(line, line_num, &mut raw);
                state = State::Header;
            }
            State::Header => {
                if is_separator(trimmed) {
                    // Separator line → comment
                    let offset = leading_spaces(line) as u32;
                    raw.push(RawToken {
                        line: line_num,
                        start: offset,
                        length: trimmed.len() as u32,
                        token_type: TT_COMMENT,
                        modifiers: 0,
                    });
                    state = State::Fields;
                } else if trimmed.to_ascii_lowercase().starts_with("recl") {
                    tokenize_recl_line(line, line_num, &mut raw);
                } else {
                    // Key line
                    tokenize_key_line(line, line_num, &mut raw);
                }
            }
            State::Fields => {
                tokenize_field_line(line, line_num, &mut raw);
            }
            State::Eof => {}
        }
    }

    encode_deltas(&mut raw)
}

fn leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn tokenize_header_line(line: &str, line_num: u32, tokens: &mut Vec<RawToken>) {
    // path, prefix, version
    let mut col = 0u32;
    for (i, part) in line.splitn(3, ',').enumerate() {
        let start = col;
        let len = part.len() as u32;
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            let trim_start = start + (part.len() - part.trim_start().len()) as u32;
            let trim_len = trimmed.len() as u32;
            let tt = match i {
                0 | 1 => TT_STRING, // path, prefix
                2 => TT_NUMBER,     // version
                _ => TT_STRING,
            };
            tokens.push(RawToken {
                line: line_num,
                start: trim_start,
                length: trim_len,
                token_type: tt,
                modifiers: 0,
            });
        }
        col = start + len + 1; // +1 for comma
    }
}

fn tokenize_key_line(line: &str, line_num: u32, tokens: &mut Vec<RawToken>) {
    // key path, field1, field2, ...
    let mut col = 0u32;
    for (i, part) in line.split(',').enumerate() {
        let start = col;
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            let trim_start = start + (part.len() - part.trim_start().len()) as u32;
            let trim_len = trimmed.len() as u32;
            let tt = if i == 0 { TT_STRING } else { TT_VARIABLE };
            tokens.push(RawToken {
                line: line_num,
                start: trim_start,
                length: trim_len,
                token_type: tt,
                modifiers: 0,
            });
        }
        col = start + part.len() as u32 + 1; // +1 for comma
    }
}

fn tokenize_recl_line(line: &str, line_num: u32, tokens: &mut Vec<RawToken>) {
    let offset = leading_spaces(line) as u32;
    let trimmed = line.trim();
    let lower = trimmed.to_ascii_lowercase();

    // "recl" keyword
    if lower.starts_with("recl") {
        tokens.push(RawToken {
            line: line_num,
            start: offset,
            length: 4,
            token_type: TT_KEYWORD,
            modifiers: 0,
        });

        // Find the number after "recl" and optional "="
        let rest = &trimmed[4..];
        let rest_trimmed = rest.trim_start_matches(|c: char| c == '=' || c.is_whitespace());
        if !rest_trimmed.is_empty() {
            let num_start = offset + 4 + (rest.len() - rest_trimmed.len()) as u32;
            let num_end = rest_trimmed
                .find(|c: char| !c.is_ascii_digit())
                .unwrap_or(rest_trimmed.len());
            if num_end > 0 {
                tokens.push(RawToken {
                    line: line_num,
                    start: num_start,
                    length: num_end as u32,
                    token_type: TT_NUMBER,
                    modifiers: 0,
                });
            }
        }
    }
}

fn tokenize_field_line(line: &str, line_num: u32, tokens: &mut Vec<RawToken>) {
    // field name, description, spec+length [, trailing comment]
    let mut col = 0u32;
    for (i, part) in line.splitn(4, ',').enumerate() {
        let start = col;
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            let trim_start = start + (part.len() - part.trim_start().len()) as u32;
            match i {
                0 => {
                    // field name → variable
                    tokens.push(RawToken {
                        line: line_num,
                        start: trim_start,
                        length: trimmed.len() as u32,
                        token_type: TT_VARIABLE,
                        modifiers: 0,
                    });
                }
                1 => {
                    // description → string
                    tokens.push(RawToken {
                        line: line_num,
                        start: trim_start,
                        length: trimmed.len() as u32,
                        token_type: TT_STRING,
                        modifiers: 0,
                    });
                }
                2 => {
                    // spec+length: split into spec keyword and numeric length
                    tokenize_spec_field(trimmed, line_num, trim_start, tokens);
                }
                3 => {
                    // trailing comment
                    tokens.push(RawToken {
                        line: line_num,
                        start: trim_start,
                        length: trimmed.len() as u32,
                        token_type: TT_COMMENT,
                        modifiers: 0,
                    });
                }
                _ => {}
            }
        }
        col = start + part.len() as u32 + 1; // +1 for comma
    }
}

/// Tokenize a combined spec+length field like "C 8", "BH 3.4", "PD 6.2".
/// Emits a keyword (or invalid) token for the spec and a number token for the length.
fn tokenize_spec_field(field: &str, line_num: u32, field_start: u32, tokens: &mut Vec<RawToken>) {
    // Find where the alphabetic spec ends and the numeric part begins
    let spec_end = field
        .find(|c: char| !c.is_ascii_alphabetic())
        .unwrap_or(field.len());
    let spec = &field[..spec_end];
    let rest = field[spec_end..].trim_start();

    if !spec.is_empty() {
        let tt = if is_valid_form(spec) {
            TT_KEYWORD
        } else {
            TT_INVALID
        };
        tokens.push(RawToken {
            line: line_num,
            start: field_start,
            length: spec.len() as u32,
            token_type: tt,
            modifiers: 0,
        });
    }

    if !rest.is_empty() {
        let num_start = field_start + (field.len() - rest.len()) as u32;
        // The rest should be a number like "8", "3.4", "6.2"
        let num_len = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        if num_len > 0 {
            tokens.push(RawToken {
                line: line_num,
                start: num_start,
                length: num_len as u32,
                token_type: TT_NUMBER,
                modifiers: 0,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// File detection helpers
// ---------------------------------------------------------------------------

pub fn is_layout_file(path: &Path) -> bool {
    // Check .lay extension
    if path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("lay"))
        .unwrap_or(false)
    {
        return true;
    }

    // Check if parent directory is "filelay" (case-insensitive)
    if let Some(parent) = path.parent() {
        if let Some(dir_name) = parent.file_name().and_then(|n| n.to_str()) {
            if dir_name.eq_ignore_ascii_case("filelay") {
                return true;
            }
        }
    }

    false
}

pub fn read_layout_file(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

// ---------------------------------------------------------------------------
// Workspace scanning
// ---------------------------------------------------------------------------

pub fn scan_workspace_layouts(folder: &tower_lsp::lsp_types::Url) -> Vec<(String, Layout)> {
    let path = match folder.to_file_path() {
        Ok(p) => p,
        Err(()) => return Vec::new(),
    };

    let mut results = Vec::new();
    for entry in walkdir::WalkDir::new(&path)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_layout_file(e.path()))
    {
        let file_path = entry.path();
        let source = match read_layout_file(file_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(layout) = parse(&source) {
            let uri = match tower_lsp::lsp_types::Url::from_file_path(file_path) {
                Ok(u) => u.to_string(),
                Err(()) => continue,
            };
            results.push((uri, layout));
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_LAYOUT: &str = "\
CUSTOMER.DAT, RCU_, 1
CUSTOMER.IX1, RCU_CUSTOMER_ID$
recl=256
----------
CUSTOMER_ID$, Customer ID, C 10
NAME$, Customer Name, C 30
BALANCE, Balance, BH 4.2
#eof#
";

    #[test]
    fn parse_standard_layout() {
        let layout = parse(SAMPLE_LAYOUT).unwrap();
        assert_eq!(layout.path, "CUSTOMER.DAT");
        assert_eq!(layout.prefix, "RCU_");
        assert_eq!(layout.version, Some(1));
        assert_eq!(layout.keys.len(), 1);
        assert_eq!(layout.keys[0].path, "CUSTOMER.IX1");
        assert_eq!(layout.keys[0].key_fields, vec!["RCU_CUSTOMER_ID$"]);
        assert_eq!(layout.record_length, Some(256));
        assert_eq!(layout.subscripts.len(), 3);
        assert_eq!(layout.subscripts[0].name, "CUSTOMER_ID$");
        assert_eq!(layout.subscripts[0].description, "Customer ID");
        assert_eq!(layout.subscripts[0].format, "C 10");
        assert_eq!(layout.subscripts[2].name, "BALANCE");
        assert_eq!(layout.subscripts[2].format, "BH 4.2");
    }

    #[test]
    fn parse_no_keys() {
        let source = "DATA.DAT, DT_, 1\n----------\nFIELD1, Desc, N 5\n";
        let layout = parse(source).unwrap();
        assert!(layout.keys.is_empty());
        assert_eq!(layout.subscripts.len(), 1);
    }

    #[test]
    fn parse_with_comments_and_eof() {
        let source = "\
! This is a comment
DATA.DAT, DT_, 1
! Another comment
----------
FIELD1, Desc, N 5
#eof#
This should be ignored
";
        let layout = parse(source).unwrap();
        assert_eq!(layout.path, "DATA.DAT");
        assert_eq!(layout.subscripts.len(), 1);
    }

    #[test]
    fn parse_empty_returns_none() {
        assert!(parse("").is_none());
        assert!(parse("  \n  \n").is_none());
    }

    // --- Semantic token tests ---

    fn collect_raw(source: &str) -> Vec<SemanticToken> {
        collect_layout_tokens(source)
    }

    #[test]
    fn token_header_line() {
        let source = "CUSTOMER.DAT, RCU_, 1\n";
        let tokens = collect_raw(source);
        // path→string, prefix→string, version→number
        assert!(tokens.len() >= 3);
        assert_eq!(tokens[0].token_type, TT_STRING); // path
        assert_eq!(tokens[1].token_type, TT_STRING); // prefix
        assert_eq!(tokens[2].token_type, TT_NUMBER); // version
    }

    #[test]
    fn token_key_line() {
        let source = "DATA.DAT, PFX_, 1\nDATA.IX1, KEY_FIELD1, KEY_FIELD2\n----------\n";
        let tokens = collect_raw(source);
        // Line 0: path, prefix, version (3 tokens)
        // Line 1: key path→string, key fields→variable (3 tokens)
        // Line 2: separator→comment (1 token)
        assert_eq!(tokens.len(), 7);
        // Key line tokens (line 1): delta_line > 0 for first
        let key_start = 3; // index into flat token list
        assert_eq!(tokens[key_start].token_type, TT_STRING); // key path
        assert_eq!(tokens[key_start + 1].token_type, TT_VARIABLE); // key field 1
        assert_eq!(tokens[key_start + 2].token_type, TT_VARIABLE); // key field 2
    }

    #[test]
    fn token_recl() {
        let source = "DATA.DAT, PFX_, 1\nrecl=128\n----------\n";
        let tokens = collect_raw(source);
        // Find the keyword and number for recl
        let recl_kw = tokens.iter().find(|t| t.token_type == TT_KEYWORD);
        assert!(recl_kw.is_some(), "should have keyword for recl");
        let recl_num = tokens.iter().filter(|t| t.token_type == TT_NUMBER).count();
        assert!(
            recl_num >= 2,
            "should have number for version and recl value"
        );
    }

    #[test]
    fn token_separator() {
        let source = "DATA.DAT, PFX_, 1\n----------\n";
        let tokens = collect_raw(source);
        let last = tokens.last().unwrap();
        assert_eq!(last.token_type, TT_COMMENT);
    }

    #[test]
    fn token_field_line() {
        let source = "DATA.DAT, PFX_, 1\n----------\nNAME$, Name, C 30, some notes\n";
        let tokens = collect_raw(source);
        // Field tokens: variable, string, keyword (C), number (30), comment
        let field_tokens: Vec<_> = tokens.iter().skip(4).collect(); // skip header(3) + separator(1)
        assert_eq!(field_tokens.len(), 5);
        assert_eq!(field_tokens[0].token_type, TT_VARIABLE); // NAME$
        assert_eq!(field_tokens[1].token_type, TT_STRING); // Name
        assert_eq!(field_tokens[2].token_type, TT_KEYWORD); // C (spec)
        assert_eq!(field_tokens[3].token_type, TT_NUMBER); // 30 (length)
        assert_eq!(field_tokens[4].token_type, TT_COMMENT); // some notes
    }

    #[test]
    fn token_field_with_decimals() {
        let source = "DATA.DAT, PFX_, 1\n----------\nAMT, Amount, BH 4.2\n";
        let tokens = collect_raw(source);
        let field_tokens: Vec<_> = tokens.iter().skip(4).collect();
        assert_eq!(field_tokens.len(), 4);
        assert_eq!(field_tokens[0].token_type, TT_VARIABLE); // AMT
        assert_eq!(field_tokens[1].token_type, TT_STRING); // Amount
        assert_eq!(field_tokens[2].token_type, TT_KEYWORD); // BH (spec)
        assert_eq!(field_tokens[3].token_type, TT_NUMBER); // 4.2 (length)
    }

    #[test]
    fn token_invalid_spec() {
        let source = "DATA.DAT, PFX_, 1\n----------\nFIELD, Desc, BADSPEC 10\n";
        let tokens = collect_raw(source);
        let has_invalid = tokens.iter().any(|t| t.token_type == TT_INVALID);
        assert!(
            has_invalid,
            "invalid form spec should produce invalid token"
        );
    }

    #[test]
    fn token_comment_line() {
        let source = "! This is a comment\nDATA.DAT, PFX_, 1\n";
        let tokens = collect_raw(source);
        assert_eq!(tokens[0].token_type, TT_COMMENT);
    }

    #[test]
    fn token_eof_and_post_eof() {
        let source = "DATA.DAT, PFX_, 1\n----------\n#eof#\nsome post-eof text\n";
        let tokens = collect_raw(source);
        // Last two tokens should be comment (eof marker and post-eof text)
        let comment_count = tokens.iter().filter(|t| t.token_type == TT_COMMENT).count();
        assert!(
            comment_count >= 3,
            "separator, #eof#, and post-eof text should all be comments"
        );
    }

    // --- File detection tests ---

    #[test]
    fn is_layout_file_lay_ext() {
        assert!(is_layout_file(Path::new("foo.lay")));
        assert!(is_layout_file(Path::new("foo.LAY")));
        assert!(is_layout_file(Path::new("/path/to/foo.lay")));
    }

    #[test]
    fn is_layout_file_filelay_parent() {
        assert!(is_layout_file(Path::new("/path/filelay/somefile")));
        assert!(is_layout_file(Path::new("filelay/data")));
    }

    #[test]
    fn is_layout_file_negative() {
        assert!(!is_layout_file(Path::new("foo.brs")));
        assert!(!is_layout_file(Path::new("foo.txt")));
        assert!(!is_layout_file(Path::new("/path/to/foo")));
        assert!(!is_layout_file(Path::new("/path/notfilelay/foo")));
    }

    // --- Layout subscript completion tests ---

    #[test]
    fn subscript_completions_basic() {
        let mut idx = LayoutIndex::new();
        idx.add(
            "file:///test.lay",
            Layout {
                path: "CUSTOMER.DAT".into(),
                prefix: "RCU_".into(),
                version: Some(1),
                keys: vec![],
                subscripts: vec![
                    LayoutSubscript {
                        name: "NAME$".into(),
                        description: "Customer Name".into(),
                        format: "C".into(),
                    },
                    LayoutSubscript {
                        name: "BALANCE".into(),
                        description: "Balance".into(),
                        format: "N".into(),
                    },
                ],
                record_length: None,
            },
        );

        let items: Vec<_> = idx.all_layouts().collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].subscripts.len(), 2);
    }

    #[test]
    fn subscript_string_vs_numeric() {
        let layout =
            parse("DATA.DAT, DT_, 1\n----------\nNAME$, Name, C 30\nBAL, Balance, N 10\n").unwrap();
        // NAME$ is a string field, BAL is numeric
        assert!(layout.subscripts[0].name.ends_with('$'));
        assert!(!layout.subscripts[1].name.ends_with('$'));
    }

    #[test]
    fn layout_index_add_remove() {
        let mut idx = LayoutIndex::new();
        let layout = parse("DATA.DAT, DT_, 1\n----------\nFIELD, Desc, N 5\n").unwrap();
        idx.add("file:///a.lay", layout);
        assert_eq!(idx.all_layouts().count(), 1);
        idx.remove("file:///a.lay");
        assert_eq!(idx.all_layouts().count(), 0);
    }

    #[test]
    fn layout_index_update() {
        let mut idx = LayoutIndex::new();
        let layout1 = parse("DATA.DAT, DT_, 1\n----------\nFIELD, Desc, N 5\n").unwrap();
        let layout2 =
            parse("OTHER.DAT, OT_, 2\n----------\nA, Desc, N 5\nB, Desc, C 10\n").unwrap();
        idx.add("file:///a.lay", layout1);
        idx.update("file:///a.lay", layout2);
        let layouts: Vec<_> = idx.all_layouts().collect();
        assert_eq!(layouts.len(), 1);
        assert_eq!(layouts[0].path, "OTHER.DAT");
        assert_eq!(layouts[0].subscripts.len(), 2);
    }
}
