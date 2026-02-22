use tower_lsp::lsp_types::{
    SemanticToken, SemanticTokenModifier, SemanticTokenType, SemanticTokensLegend,
};
use tree_sitter::Tree;

pub const TOKEN_TYPES: &[SemanticTokenType] = &[
    SemanticTokenType::FUNCTION,          // 0
    SemanticTokenType::VARIABLE,          // 1
    SemanticTokenType::PARAMETER,         // 2
    SemanticTokenType::KEYWORD,           // 3
    SemanticTokenType::COMMENT,           // 4
    SemanticTokenType::STRING,            // 5
    SemanticTokenType::NUMBER,            // 6
    SemanticTokenType::PROPERTY,          // 7
    SemanticTokenType::ENUM_MEMBER,       // 8
    SemanticTokenType::OPERATOR,          // 9
    SemanticTokenType::new("lineNumber"), // 10
    SemanticTokenType::new("invalid"),    // 11
];

pub const TOKEN_MODIFIERS: &[SemanticTokenModifier] = &[
    SemanticTokenModifier::DECLARATION,        // bit 0
    SemanticTokenModifier::DEFAULT_LIBRARY,    // bit 1
    SemanticTokenModifier::DEFINITION,         // bit 2
    SemanticTokenModifier::new("controlFlow"), // bit 3
];

pub fn legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: TOKEN_TYPES.to_vec(),
        token_modifiers: TOKEN_MODIFIERS.to_vec(),
    }
}

pub(crate) struct RawToken {
    pub line: u32,
    pub start: u32,
    pub length: u32,
    pub token_type: u32,
    pub modifiers: u32,
}

pub fn collect_tokens(tree: &Tree, source: &str) -> Vec<SemanticToken> {
    let mut raw = Vec::new();
    walk_node(tree.root_node(), source, false, false, &mut raw);
    encode_deltas(&mut raw)
}

fn walk_node(
    node: tree_sitter::Node,
    source: &str,
    in_parameter: bool,
    in_dim: bool,
    tokens: &mut Vec<RawToken>,
) {
    let kind = node.kind();
    let is_named = node.is_named();

    // Determine if this node sets the parameter context for children
    let child_in_parameter = in_parameter
        || matches!(
            kind,
            "parameter_list" | "required_parameter" | "optional_parameter" | "parameter"
        );

    let child_in_dim = in_dim || kind == "dim_statement";

    // Emit a keyword token for the hidden `mat` prefix in array nodes and mat statements
    if matches!(kind, "numberarray" | "stringarray" | "mat_statement") {
        emit_mat_keyword(node, source, tokens);
    }

    if let Some((token_type, modifiers)) = classify_node(kind, is_named, node, in_parameter, in_dim)
    {
        // String/template_string nodes with a range child (e.g. "test"(1:2)) —
        // emit the string token only for the quoted portion, then recurse so the
        // range children get their own (number) tokens.
        if matches!(kind, "string" | "template_string")
            && (0..node.named_child_count())
                .any(|i| node.named_child(i).is_some_and(|c| c.kind() == "range"))
        {
            let start = node.start_position();
            // Find the first range child and end the string token there
            let mut cursor = node.walk();
            let range_start = node
                .children(&mut cursor)
                .find(|c| c.kind() == "range")
                .map(|c| c.start_position());
            if let Some(rs) = range_start {
                // Emit just the quoted string portion (up to the opening paren)
                if start.row == rs.row && rs.column > start.column {
                    tokens.push(RawToken {
                        line: start.row as u32,
                        start: start.column as u32,
                        length: (rs.column - start.column) as u32,
                        token_type,
                        modifiers,
                    });
                }
            }
            // Recurse into children (range will emit number tokens)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk_node(child, source, child_in_parameter, child_in_dim, tokens);
            }
            return;
        }

        let start = node.start_position();
        let end = node.end_position();

        if start.row == end.row {
            // Single-line token
            let length = (end.column - start.column) as u32;
            if length > 0 {
                tokens.push(RawToken {
                    line: start.row as u32,
                    start: start.column as u32,
                    length,
                    token_type,
                    modifiers,
                });
            }
        } else {
            // Multi-line token (e.g. multiline comments) — emit one token per line
            emit_multiline_token(source, start, end, token_type, modifiers, tokens);
        }

        // For leaf-like tokens (comments, strings, numbers, etc.), don't recurse into children
        if is_leaf_token(kind) {
            return;
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_node(child, source, child_in_parameter, child_in_dim, tokens);
    }
}

/// Returns true for node kinds where we should NOT recurse into children
/// after emitting a token (the entire node text is one semantic unit).
fn is_leaf_token(kind: &str) -> bool {
    matches!(
        kind,
        "comment"
            | "multiline_comment"
            | "doc_comment"
            | "string"
            | "template_string"
            | "number"
            | "line_number"
            | "label"
            | "label_reference"
            | "line_reference"
            | "error_condition"
    )
}

pub(crate) fn classify_node(
    kind: &str,
    is_named: bool,
    node: tree_sitter::Node,
    in_parameter: bool,
    in_dim: bool,
) -> Option<(u32, u32)> {
    match kind {
        "function_name" => {
            let mut modifiers = 0u32;
            if let Some(parent) = node.parent() {
                match parent.kind() {
                    "numeric_function_definition" | "string_function_definition" => {
                        modifiers |= 1 << 0; // declaration
                    }
                    "numeric_system_function" | "string_system_function" => {
                        modifiers |= 1 << 1; // defaultLibrary
                    }
                    _ => {}
                }
            }
            Some((0, modifiers)) // function
        }
        "numberidentifier" | "stringidentifier" => {
            if in_parameter {
                Some((2, 0)) // parameter
            } else {
                let modifiers = if in_dim { 1 << 0 } else { 0 }; // declaration
                Some((1, modifiers)) // variable
            }
        }
        "statement" if !is_named => Some((3, 0)), // keyword (statement)
        "keyword" if !is_named => Some((3, 1 << 3)), // keyword + controlFlow
        "comment" | "multiline_comment" | "doc_comment" => Some((4, 0)), // comment
        "string" | "template_string" => Some((5, 0)), // string
        "number" | "int" => Some((6, 0)),         // number
        "0" | "1" if !is_named && is_inside(node, "option_statement") => Some((6, 0)), // option base 0/1
        "line_number" => Some((10, 0)),                                                // lineNumber
        "label" => Some((7, 1 << 2)), // property + definition
        "label_reference" | "line_reference" => Some((7, 0)), // property
        "error_condition" => Some((8, 0)), // enumMember
        "*" if !is_named && in_dim => Some((9, 0)), // operator (length modifier)
        _ => None,
    }
}

/// Check if a node has an ancestor with the given kind.
pub(crate) fn is_inside(node: tree_sitter::Node, ancestor_kind: &str) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.kind() == ancestor_kind {
            return true;
        }
        current = n.parent();
    }
    false
}

/// Detect and emit a keyword token for the hidden `mat` prefix in array nodes.
/// The grammar's `_mat` rule is anonymous so tree-sitter doesn't create a child
/// node for it — we check if the source text before the first child is `mat`.
fn emit_mat_keyword(node: tree_sitter::Node, source: &str, tokens: &mut Vec<RawToken>) {
    let node_start = node.start_byte();
    let first_child_start = match node.child(0) {
        Some(c) => c.start_byte(),
        None => return,
    };
    if first_child_start <= node_start {
        return;
    }
    let prefix = &source[node_start..first_child_start];
    let trimmed = prefix.trim_end();
    if trimmed.eq_ignore_ascii_case("mat") {
        let pos = node.start_position();
        tokens.push(RawToken {
            line: pos.row as u32,
            start: pos.column as u32,
            length: 3,         // "mat" is always 3 chars
            token_type: 3,     // keyword
            modifiers: 1 << 3, // controlFlow
        });
    }
}

fn emit_multiline_token(
    source: &str,
    start: tree_sitter::Point,
    end: tree_sitter::Point,
    token_type: u32,
    modifiers: u32,
    tokens: &mut Vec<RawToken>,
) {
    let lines: Vec<&str> = source.lines().collect();
    for line_idx in start.row..=end.row {
        let col_start = if line_idx == start.row {
            start.column
        } else {
            0
        };
        let col_end = if line_idx == end.row {
            end.column
        } else {
            lines.get(line_idx).map_or(0, |l| l.len())
        };
        if col_end > col_start {
            tokens.push(RawToken {
                line: line_idx as u32,
                start: col_start as u32,
                length: (col_end - col_start) as u32,
                token_type,
                modifiers,
            });
        }
    }
}

pub(crate) fn encode_deltas(tokens: &mut [RawToken]) -> Vec<SemanticToken> {
    // Sort by line, then by start column
    tokens.sort_by(|a, b| a.line.cmp(&b.line).then(a.start.cmp(&b.start)));

    let mut result = Vec::with_capacity(tokens.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for tok in tokens.iter() {
        let delta_line = tok.line - prev_line;
        let delta_start = if delta_line == 0 {
            tok.start - prev_start
        } else {
            tok.start
        };

        result.push(SemanticToken {
            delta_line,
            delta_start,
            length: tok.length,
            token_type: tok.token_type,
            token_modifiers_bitset: tok.modifiers,
        });

        prev_line = tok.line;
        prev_start = tok.start;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_and_collect(source: &str) -> Vec<SemanticToken> {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        collect_tokens(&tree, source)
    }

    #[test]
    fn keyword_tokens() {
        let tokens = parse_and_collect("let x = 1\n");
        // Should have at least a keyword (let), variable (x), number (1)
        assert!(tokens.len() >= 3);
        // First token should be keyword "let" at col 0
        assert_eq!(tokens[0].token_type, 3); // keyword
        assert_eq!(tokens[0].delta_line, 0);
        assert_eq!(tokens[0].delta_start, 0);
    }

    #[test]
    fn function_call_tokens() {
        let source = "let x = Val(\"123\")\n";
        let tokens = parse_and_collect(source);
        // Should contain a function token for "Val"
        let has_function = tokens.iter().any(|t| t.token_type == 0); // function
        assert!(has_function);
    }

    #[test]
    fn comment_token() {
        let source = "! this is a comment\n";
        let tokens = parse_and_collect(source);
        let has_comment = tokens.iter().any(|t| t.token_type == 4); // comment
        assert!(has_comment);
    }

    #[test]
    fn string_token() {
        let source = "let x$ = \"hello\"\n";
        let tokens = parse_and_collect(source);
        let has_string = tokens.iter().any(|t| t.token_type == 5); // string
        assert!(has_string);
    }

    #[test]
    fn delta_encoding_same_line() {
        // Two tokens on the same line should have delta_line=0
        let source = "let x = 1\n";
        let tokens = parse_and_collect(source);
        // All tokens on line 0
        for tok in &tokens {
            assert_eq!(tok.delta_line, 0, "all tokens should be on line 0");
        }
    }

    #[test]
    fn delta_encoding_multiple_lines() {
        let source = "let x = 1\nlet y = 2\n";
        let tokens = parse_and_collect(source);
        // There should be some tokens with delta_line > 0
        let has_line_change = tokens.iter().any(|t| t.delta_line > 0);
        assert!(has_line_change);
    }

    #[test]
    fn mat_keyword_token() {
        let source = "print mat x\n";
        let tokens = parse_and_collect(source);
        // Should have keyword "print", keyword+controlFlow "mat", variable "x"
        let mat_token = tokens
            .iter()
            .find(|t| t.token_type == 3 && t.token_modifiers_bitset == (1 << 3));
        assert!(mat_token.is_some(), "mat should be keyword+controlFlow");
    }

    #[test]
    fn mat_statement_keyword_token() {
        let source = "00100 mat x$(10)\n";
        let tokens = parse_and_collect(source);
        // The "mat" in a mat_statement should also be keyword+controlFlow
        let mat_token = tokens
            .iter()
            .find(|t| t.token_type == 3 && t.token_modifiers_bitset == (1 << 3));
        assert!(mat_token.is_some(), "mat in mat_statement should be keyword+controlFlow");
    }

    #[test]
    fn empty_source_no_tokens() {
        let tokens = parse_and_collect("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn option_base_number_token() {
        let tokens = parse_and_collect("01000    option base 1\n");
        // The "1" should be classified as a number (type 6)
        let has_number = tokens.iter().any(|t| t.token_type == 6);
        assert!(has_number, "option base 1 should have a number token");
    }

    #[test]
    fn string_with_range_splits_tokens() {
        // "test"(1:2) — the range numbers should NOT be string tokens
        let tokens = parse_and_collect("00100 print \"test\"(1:2)\n");
        let string_tokens: Vec<_> = tokens.iter().filter(|t| t.token_type == 5).collect();
        let number_tokens: Vec<_> = tokens.iter().filter(|t| t.token_type == 6).collect();
        assert!(!string_tokens.is_empty(), "should have a string token");
        // The range contains two numbers (1 and 2) that should be number tokens
        assert!(
            number_tokens.len() >= 2,
            "range numbers should not be colored as strings, got {} number tokens",
            number_tokens.len()
        );
    }
}
