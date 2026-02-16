use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity};
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

use crate::parser;

pub fn collect_function_diagnostics(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let mut diagnostics = check_missing_fnend(tree, source);
    diagnostics.extend(check_duplicate_functions(tree, source));
    diagnostics
}

/// Extract the `function_name` child node from a `def_statement` node.
fn function_name_node(def_node: Node) -> Option<Node> {
    let mut cursor = def_node.walk();
    for child in def_node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "string_function_definition" || kind == "numeric_function_definition" {
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if grandchild.kind() == "function_name" {
                    return Some(grandchild);
                }
            }
        }
    }
    None
}

/// Check whether a `def_statement` is an inline function (has `assignment_op`
/// in its function definition child, e.g. `DEF fnFoo(X)=X*2`).
fn is_inline_def(def_node: Node) -> bool {
    let mut cursor = def_node.walk();
    for child in def_node.children(&mut cursor) {
        let kind = child.kind();
        if kind == "string_function_definition" || kind == "numeric_function_definition" {
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if grandchild.kind() == "assignment_op" {
                    return true;
                }
            }
        }
    }
    false
}

fn check_missing_fnend(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let language = tree.language();
    let query = match Query::new(
        &language,
        "(def_statement) @def (fnend_statement) @fnend (end_def_statement) @enddef",
    ) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };

    // Collect all relevant nodes: (start_byte, pattern_index, node info)
    enum Entry {
        Def {
            range: tower_lsp::lsp_types::Range,
            name: String,
        },
        Close,
    }

    let mut entries: Vec<(usize, Entry)> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    while let Some(m) = matches.next() {
        let node = m.captures[0].node;
        match m.pattern_index {
            0 => {
                // def_statement — skip inline functions
                if is_inline_def(node) {
                    // Inline defs still close any open function
                    entries.push((node.start_byte(), Entry::Close));
                    continue;
                }
                let name = function_name_node(node)
                    .and_then(|n| n.utf8_text(source.as_bytes()).ok())
                    .unwrap_or("")
                    .to_string();
                entries.push((
                    node.start_byte(),
                    Entry::Def {
                        range: parser::node_range(node),
                        name,
                    },
                ));
            }
            1 | 2 => {
                // fnend_statement or end_def_statement
                entries.push((node.start_byte(), Entry::Close));
            }
            _ => {}
        }
    }

    entries.sort_by_key(|(byte, _)| *byte);

    let mut diagnostics = Vec::new();
    let mut open_def: Option<(tower_lsp::lsp_types::Range, String)> = None;

    for (_, entry) in entries {
        match entry {
            Entry::Def { range, name } => {
                if let Some((prev_range, prev_name)) = open_def.take() {
                    diagnostics.push(Diagnostic {
                        range: prev_range,
                        severity: Some(DiagnosticSeverity::ERROR),
                        message: format!("Function '{prev_name}' is missing FNEND"),
                        ..Default::default()
                    });
                }
                open_def = Some((range, name));
            }
            Entry::Close => {
                open_def = None;
            }
        }
    }

    if let Some((range, name)) = open_def {
        diagnostics.push(Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::ERROR),
            message: format!("Function '{name}' is missing FNEND"),
            ..Default::default()
        });
    }

    diagnostics
}

fn check_duplicate_functions(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let language = tree.language();
    let query = match Query::new(&language, "(def_statement) @def") {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());

    // Collect (lowercase_name, display_name, function_name_range) in document order
    let mut functions: Vec<(String, String, tower_lsp::lsp_types::Range)> = Vec::new();

    while let Some(m) = matches.next() {
        let node = m.captures[0].node;
        if let Some(name_node) = function_name_node(node) {
            let name = name_node
                .utf8_text(source.as_bytes())
                .unwrap_or("")
                .to_string();
            functions.push((name.to_ascii_lowercase(), name, parser::node_range(name_node)));
        }
    }

    let mut diagnostics = Vec::new();
    let mut seen: std::collections::HashMap<String, bool> = std::collections::HashMap::new();

    for (key, name, range) in &functions {
        if seen.contains_key(key) {
            diagnostics.push(Diagnostic {
                range: *range,
                severity: Some(DiagnosticSeverity::ERROR),
                message: format!("Function '{name}' is already defined in this file"),
                ..Default::default()
            });
        } else {
            seen.insert(key.clone(), true);
        }
    }

    diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(source: &str) -> Tree {
        let mut p = parser::new_parser();
        parser::parse(&mut p, source, None).expect("parse failed")
    }

    #[test]
    fn missing_fnend_basic() {
        let source = "def fnFoo(X)\nlet Y=X*2\n";
        let tree = parse(source);
        let diags = check_missing_fnend(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnFoo"));
        assert!(diags[0].message.contains("missing FNEND"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn inline_function_no_diagnostic() {
        let source = "def fnFoo(X)=X*2\n";
        let tree = parse(source);
        let diags = check_missing_fnend(&tree, source);
        assert!(diags.is_empty(), "inline function should not need FNEND");
    }

    #[test]
    fn fnend_closes_function() {
        let source = "def fnFoo(X)\nlet Y=X*2\nfnend\n";
        let tree = parse(source);
        let diags = check_missing_fnend(&tree, source);
        assert!(diags.is_empty(), "FNEND should close the function");
    }

    #[test]
    fn end_def_closes_function() {
        let source = "def fnFoo(X)\nlet Y=X*2\nend def\n";
        let tree = parse(source);
        let diags = check_missing_fnend(&tree, source);
        assert!(diags.is_empty(), "END DEF should close the function");
    }

    #[test]
    fn nested_missing_fnend() {
        let source = "def fnFoo(X)\nlet Y=X\ndef fnBar(Z)\nlet W=Z\nfnend\n";
        let tree = parse(source);
        let diags = check_missing_fnend(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0].message.contains("fnFoo"),
            "first function should be flagged"
        );
    }

    #[test]
    fn duplicate_function() {
        let source = "def fnFoo(X)=X\ndef fnFoo(Y)=Y\n";
        let tree = parse(source);
        let diags = check_duplicate_functions(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnFoo"));
        assert!(diags[0].message.contains("already defined"));
    }

    #[test]
    fn duplicate_case_insensitive() {
        let source = "def fnFoo(X)=X\ndef FNFOO(Y)=Y\n";
        let tree = parse(source);
        let diags = check_duplicate_functions(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("already defined"));
    }

    #[test]
    fn no_duplicate_different_names() {
        let source = "def fnFoo(X)=X\ndef fnBar(Y)=Y\n";
        let tree = parse(source);
        let diags = check_duplicate_functions(&tree, source);
        assert!(diags.is_empty());
    }
}
