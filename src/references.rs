use tower_lsp::lsp_types::Range;
use tree_sitter::Tree;

use crate::parser::{node_at_position, run_query, QueryResult};

const SUPPORTED_KINDS: &[&str] = &[
    "function_name",
    "label",
    "label_reference",
    "line_number",
    "line_reference",
    "stringidentifier",
    "numberidentifier",
];

pub fn find_references(tree: &Tree, source: &str, line: usize, character: usize) -> Vec<Range> {
    let mut node = match node_at_position(tree, line, character) {
        Some(n) => n,
        None => return Vec::new(),
    };

    // When cursor is at the end of a token, tree-sitter returns the parent/next node.
    // Fall back to the previous column to find the intended token.
    if !SUPPORTED_KINDS.contains(&node.kind()) && character > 0 {
        if let Some(n) = node_at_position(tree, line, character - 1) {
            if SUPPORTED_KINDS.contains(&n.kind()) {
                node = n;
            }
        }
    }

    match node.kind() {
        "function_name" => find_function_refs(&node, tree, source),
        "label" | "label_reference" => find_label_refs(&node, tree, source),
        "line_number" | "line_reference" => find_line_refs(&node, tree, source),
        "stringidentifier" | "numberidentifier" => find_variable_refs(&node, tree, source),
        _ => Vec::new(),
    }
}

fn escape_for_query(name: &str) -> String {
    let mut result = String::new();
    for ch in name.chars() {
        if ch == '$' {
            result.push_str("\\$");
        } else if ch.is_ascii_alphabetic() {
            result.push('[');
            result.push(ch.to_ascii_uppercase());
            result.push(ch.to_ascii_lowercase());
            result.push(']');
        } else {
            result.push(ch);
        }
    }
    result
}

fn find_function_refs(node: &tree_sitter::Node, tree: &Tree, source: &str) -> Vec<Range> {
    let name = node.utf8_text(source.as_bytes()).unwrap_or("");
    let escaped = escape_for_query(name);
    let query = format!("((function_name) @name (#match? @name \"^{escaped}$\"))");
    run_query(&query, tree.root_node(), source)
        .into_iter()
        .map(|r| r.range)
        .collect()
}

fn find_label_refs(node: &tree_sitter::Node, tree: &Tree, source: &str) -> Vec<Range> {
    let text = node.utf8_text(source.as_bytes()).unwrap_or("");
    let name = text.trim_end_matches(':');
    let escaped = escape_for_query(name);
    let query = format!(
        "((label) @label (#match? @label \"^{escaped}:$\"))\n\
         ((label_reference) @label_ref (#match? @label_ref \"^{escaped}$\"))"
    );
    run_query(&query, tree.root_node(), source)
        .into_iter()
        .map(|r| {
            if r.kind == "label" {
                // Exclude trailing colon from the range
                Range {
                    start: r.range.start,
                    end: tower_lsp::lsp_types::Position {
                        line: r.range.end.line,
                        character: r.range.end.character.saturating_sub(1),
                    },
                }
            } else {
                r.range
            }
        })
        .collect()
}

fn find_line_refs(node: &tree_sitter::Node, tree: &Tree, source: &str) -> Vec<Range> {
    let text = node.utf8_text(source.as_bytes()).unwrap_or("");
    let target_num: i64 = match text.trim().parse() {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };

    let query = "((line_number) @ln) ((line_reference) @lr)";
    run_query(query, tree.root_node(), source)
        .into_iter()
        .filter(|r| {
            r.text
                .trim()
                .parse::<i64>()
                .map(|n| n == target_num)
                .unwrap_or(false)
        })
        .map(|r| r.range)
        .collect()
}

fn find_variable_refs(node: &tree_sitter::Node, tree: &Tree, source: &str) -> Vec<Range> {
    let name = node.utf8_text(source.as_bytes()).unwrap_or("");
    let parent = match node.parent() {
        Some(p) => p,
        None => return Vec::new(),
    };
    let parent_type = parent.kind();
    let escaped = escape_for_query(name);
    let query = format!("(({parent_type} name: (_) @name (#match? @name \"^{escaped}$\")))");
    let results = run_query(&query, tree.root_node(), source);
    filter_by_scope(node, tree, source, results)
}

struct FunctionRange {
    def_start_byte: usize,
    body_end_byte: usize,
}

fn get_function_ranges(tree: &Tree, source: &str) -> Vec<FunctionRange> {
    let query = "(line (def_statement) @def)\n(fnend_statement) @fnend";
    let results = run_query(query, tree.root_node(), source);

    let mut ranges = Vec::new();
    let mut pending_def: Option<&QueryResult> = None;

    for r in &results {
        match r.kind.as_str() {
            "def_statement" => {
                pending_def = Some(r);
            }
            "fnend_statement" => {
                if let Some(def) = pending_def.take() {
                    ranges.push(FunctionRange {
                        def_start_byte: def.start_byte,
                        body_end_byte: r.start_byte,
                    });
                }
            }
            _ => {}
        }
    }

    ranges
}

fn is_param_of_function(
    node: &tree_sitter::Node,
    def_start_byte: usize,
    body_end_byte: usize,
    tree: &Tree,
    source: &str,
) -> bool {
    let name = node.utf8_text(source.as_bytes()).unwrap_or("");
    let parent_type = match node.parent() {
        Some(p) => p.kind().to_string(),
        None => return false,
    };

    let query = "(parameter) @param";
    let results = run_query(query, tree.root_node(), source);

    for r in &results {
        // Only consider parameters within this function's def_statement
        if r.start_byte < def_start_byte || r.start_byte > body_end_byte {
            continue;
        }
        // Walk the parameter node to find the identifier
        let param_node = match node_at_position(
            tree,
            r.range.start.line as usize,
            r.range.start.character as usize,
        ) {
            Some(n) => n,
            None => continue,
        };
        // Find matching identifier within the parameter subtree
        if has_matching_identifier(&param_node, &parent_type, name, source) {
            return true;
        }
    }
    false
}

fn has_matching_identifier(
    param_node: &tree_sitter::Node,
    parent_type: &str,
    name: &str,
    source: &str,
) -> bool {
    // Walk the parameter subtree looking for an identifier with matching parent type and name
    let mut cursor = param_node.walk();
    let mut found = false;

    // DFS through the subtree
    'outer: loop {
        let n = cursor.node();
        if (n.kind() == "stringidentifier" || n.kind() == "numberidentifier")
            && n.parent().map(|p| p.kind()) == Some(parent_type)
        {
            let node_text = n.utf8_text(source.as_bytes()).unwrap_or("");
            if node_text.eq_ignore_ascii_case(name) {
                found = true;
                break;
            }
        }

        if cursor.goto_first_child() {
            continue;
        }
        loop {
            if cursor.goto_next_sibling() {
                continue 'outer;
            }
            if !cursor.goto_parent() {
                break 'outer;
            }
        }
    }

    found
}

fn in_function(byte_offset: usize, ranges: &[FunctionRange]) -> Option<usize> {
    ranges
        .iter()
        .position(|r| byte_offset >= r.def_start_byte && byte_offset <= r.body_end_byte)
}

fn filter_by_scope(
    node: &tree_sitter::Node,
    tree: &Tree,
    source: &str,
    results: Vec<QueryResult>,
) -> Vec<Range> {
    let fn_ranges = get_function_ranges(tree, source);
    let cursor_byte = node.start_byte();

    let cursor_fn_idx = in_function(cursor_byte, &fn_ranges);
    let is_cursor_param = if let Some(idx) = cursor_fn_idx {
        let fr = &fn_ranges[idx];
        is_param_of_function(node, fr.def_start_byte, fr.body_end_byte, tree, source)
    } else {
        false
    };

    if is_cursor_param {
        // Cursor is on a parameter — keep only refs inside the same function body
        let fr = &fn_ranges[cursor_fn_idx.unwrap()];
        results
            .into_iter()
            .filter(|r| r.start_byte >= fr.def_start_byte && r.start_byte <= fr.body_end_byte)
            .map(|r| r.range)
            .collect()
    } else {
        // Cursor is NOT a parameter — exclude refs that are parameters of any function
        results
            .into_iter()
            .filter(|r| {
                if let Some(ref_node) = node_at_position(
                    tree,
                    r.range.start.line as usize,
                    r.range.start.character as usize,
                ) {
                    if let Some(idx) = in_function(r.start_byte, &fn_ranges) {
                        let fr = &fn_ranges[idx];
                        !is_param_of_function(
                            &ref_node,
                            fr.def_start_byte,
                            fr.body_end_byte,
                            tree,
                            source,
                        )
                    } else {
                        true
                    }
                } else {
                    true
                }
            })
            .map(|r| r.range)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_and_find(source: &str, line: usize, character: usize) -> Vec<Range> {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        find_references(&tree, source, line, character)
    }

    #[test]
    fn print_variable_references() {
        let source = "print test\nprint test\n";
        let refs = parse_and_find(source, 0, 6);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn function_references() {
        let source = "def fnTest(x)\nlet y = fnTest(1)\nfnend\n";
        // cursor on `fnTest` in the def statement (line 0, col 4)
        let refs = parse_and_find(source, 0, 4);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn function_references_case_insensitive() {
        let source = "def fnTest(x)\nlet y = FNTEST(1)\nfnend\n";
        let refs = parse_and_find(source, 0, 4);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn label_references() {
        let source = "MYLOOP:\nlet x = 1\ngoto MYLOOP\n";
        // cursor on `MYLOOP:` label (line 0, col 0)
        let refs = parse_and_find(source, 0, 0);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn line_number_references() {
        let source = "00100 let x = 1\n00200 goto 100\n";
        // cursor on line number 00100 (line 0, col 2)
        let refs = parse_and_find(source, 0, 2);
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn variable_scope_param_only_in_function() {
        let source = "\
let X = 1
def fnFoo(X)
let Y = X + 1
fnend
let Z = X + 2
";
        // cursor on X in the function body (line 2, find the X)
        // First find where X is in "let Y = X + 1" — line 2
        let x_col = source.lines().nth(2).unwrap().find('X').unwrap();
        let refs = parse_and_find(source, 2, x_col);
        // X inside the function is a parameter — should only find refs inside the function
        assert_eq!(refs.len(), 2); // param X and body X
    }

    #[test]
    fn variable_scope_non_param_excludes_params() {
        let source = "\
let X = 1
def fnFoo(X)
let Y = X + 1
fnend
let Z = X + 2
";
        // cursor on X outside the function (line 0)
        let x_col = source.lines().next().unwrap().find('X').unwrap();
        let refs = parse_and_find(source, 0, x_col);
        // X outside the function — should exclude param occurrences
        assert_eq!(refs.len(), 2); // line 0 and line 4
    }

    #[test]
    fn no_refs_for_unknown_node() {
        let source = "let x = 1\n";
        // cursor on a number literal
        let refs = parse_and_find(source, 0, 8);
        assert!(refs.is_empty());
    }

    #[test]
    fn cursor_at_end_of_token() {
        let source = "def fnTest(x)\nlet y = fnTest(1)\nfnend\n";
        // cursor right after `fnTest` (col 10 = one past the last char)
        let refs = parse_and_find(source, 0, 10);
        assert_eq!(refs.len(), 2);
    }
}
