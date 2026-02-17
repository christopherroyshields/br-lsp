use tower_lsp::lsp_types::Range;
use tree_sitter::Tree;

use crate::parser::{node_at_position, run_query};
use crate::references::escape_for_query;

const SUPPORTED_KINDS: &[&str] = &[
    "function_name",
    "label_reference",
    "line_reference",
    "stringidentifier",
    "numberidentifier",
];

pub enum DefinitionResult {
    Found(Range),
    LookupFunction(String),
    None,
}

pub fn find_definition(
    tree: &Tree,
    source: &str,
    line: usize,
    character: usize,
) -> DefinitionResult {
    let mut node = match node_at_position(tree, line, character) {
        Some(n) => n,
        None => return DefinitionResult::None,
    };

    // End-of-token fallback (same pattern as references)
    if !SUPPORTED_KINDS.contains(&node.kind()) && character > 0 {
        if let Some(n) = node_at_position(tree, line, character - 1) {
            if SUPPORTED_KINDS.contains(&n.kind()) {
                node = n;
            }
        }
    }

    match node.kind() {
        "function_name" => {
            // Skip system functions
            if let Some(parent) = node.parent() {
                if parent.kind() == "numeric_system_function"
                    || parent.kind() == "string_system_function"
                {
                    return DefinitionResult::None;
                }
            }
            let name = node.utf8_text(source.as_bytes()).unwrap_or("");
            find_function_def(tree, source, name)
        }
        "label_reference" => {
            let name = node.utf8_text(source.as_bytes()).unwrap_or("");
            find_label_def(tree, source, name)
        }
        "line_reference" => {
            let text = node.utf8_text(source.as_bytes()).unwrap_or("");
            find_line_def(tree, source, text)
        }
        "stringidentifier" | "numberidentifier" => {
            let name = node.utf8_text(source.as_bytes()).unwrap_or("");
            find_dim_def(tree, source, name)
        }
        _ => DefinitionResult::None,
    }
}

fn find_function_def(tree: &Tree, source: &str, name: &str) -> DefinitionResult {
    let escaped = escape_for_query(name);
    let query = format!(
        "(def_statement [(numeric_function_definition (function_name) @name) (string_function_definition (function_name) @name)] (#match? @name \"^{escaped}$\"))"
    );
    let results = run_query(&query, tree.root_node(), source);
    if let Some(r) = results.first() {
        DefinitionResult::Found(r.range)
    } else {
        DefinitionResult::LookupFunction(name.to_string())
    }
}

fn find_label_def(tree: &Tree, source: &str, name: &str) -> DefinitionResult {
    let escaped = escape_for_query(name);
    let query = format!("((label) @label (#match? @label \"^{escaped}:$\"))");
    let results = run_query(&query, tree.root_node(), source);
    if let Some(r) = results.first() {
        // Exclude trailing colon from the range
        let range = Range {
            start: r.range.start,
            end: tower_lsp::lsp_types::Position {
                line: r.range.end.line,
                character: r.range.end.character.saturating_sub(1),
            },
        };
        DefinitionResult::Found(range)
    } else {
        DefinitionResult::None
    }
}

fn find_line_def(tree: &Tree, source: &str, text: &str) -> DefinitionResult {
    let target_num: i64 = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => return DefinitionResult::None,
    };

    let query = "((line_number) @ln)";
    let results = run_query(query, tree.root_node(), source);
    for r in &results {
        if r.text
            .trim()
            .parse::<i64>()
            .map(|n| n == target_num)
            .unwrap_or(false)
        {
            return DefinitionResult::Found(r.range);
        }
    }
    DefinitionResult::None
}

fn find_dim_def(tree: &Tree, source: &str, name: &str) -> DefinitionResult {
    let escaped = escape_for_query(name);
    let query = format!(
        concat!(
            "(dim_statement [(stringreference name: (_) @name)",
            " (numberreference name: (_) @name)",
            " (stringarray name: (_) @name)",
            " (numberarray name: (_) @name)]",
            " (#match? @name \"^{escaped}$\"))",
        ),
        escaped = escaped,
    );
    let results = run_query(&query, tree.root_node(), source);
    if let Some(r) = results.first() {
        DefinitionResult::Found(r.range)
    } else {
        DefinitionResult::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_and_find(source: &str, line: usize, character: usize) -> DefinitionResult {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        find_definition(&tree, source, line, character)
    }

    #[test]
    fn function_def_same_file() {
        let source = "def fnAdd(A, B) = A + B\nlet x = fnAdd(1, 2)\n";
        // Cursor on fnAdd call (line 1)
        let col = source.lines().nth(1).unwrap().find("fnAdd").unwrap();
        match parse_and_find(source, 1, col) {
            DefinitionResult::Found(range) => {
                assert_eq!(range.start.line, 0);
            }
            _ => panic!("Expected Found"),
        }
    }

    #[test]
    fn function_def_cross_file() {
        let source = "let x = fnMissing(1)\n";
        let col = source.find("fnMissing").unwrap();
        match parse_and_find(source, 0, col) {
            DefinitionResult::LookupFunction(name) => {
                assert_eq!(name, "fnMissing");
            }
            _ => panic!("Expected LookupFunction"),
        }
    }

    #[test]
    fn function_def_case_insensitive() {
        let source = "def fnTest(X) = X\nlet y = FNTEST(1)\n";
        let col = source.lines().nth(1).unwrap().find("FNTEST").unwrap();
        match parse_and_find(source, 1, col) {
            DefinitionResult::Found(range) => {
                assert_eq!(range.start.line, 0);
            }
            _ => panic!("Expected Found"),
        }
    }

    #[test]
    fn label_def() {
        let source = "MYLOOP:\nlet x = 1\ngoto MYLOOP\n";
        let col = source.lines().nth(2).unwrap().find("MYLOOP").unwrap();
        match parse_and_find(source, 2, col) {
            DefinitionResult::Found(range) => {
                assert_eq!(range.start.line, 0);
                // Should exclude trailing colon
                assert_eq!(range.end.character, 6);
            }
            _ => panic!("Expected Found"),
        }
    }

    #[test]
    fn line_def() {
        let source = "00100 let x = 1\n00200 goto 100\n";
        // Cursor on "100" in goto statement (line_reference)
        let line1 = source.lines().nth(1).unwrap();
        let col = line1.find("100").unwrap();
        match parse_and_find(source, 1, col) {
            DefinitionResult::Found(range) => {
                assert_eq!(range.start.line, 0);
            }
            _ => panic!("Expected Found"),
        }
    }

    #[test]
    fn dim_variable_def() {
        let source = "dim X$*30\nprint X$\n";
        let col = source.lines().nth(1).unwrap().find("X$").unwrap();
        match parse_and_find(source, 1, col + 1) {
            DefinitionResult::Found(range) => {
                assert_eq!(range.start.line, 0);
            }
            _ => panic!("Expected Found"),
        }
    }

    #[test]
    fn system_function_returns_none() {
        let source = "let x = Str$(42)\n";
        let col = source.find("Str$").unwrap();
        match parse_and_find(source, 0, col) {
            DefinitionResult::None => {}
            _ => panic!("Expected None for system function"),
        }
    }

    #[test]
    fn no_definition_for_unknown() {
        let source = "let x = 1\n";
        match parse_and_find(source, 0, 8) {
            DefinitionResult::None => {}
            _ => panic!("Expected None"),
        }
    }
}
