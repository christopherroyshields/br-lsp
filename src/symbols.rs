use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
use tree_sitter::Tree;

use crate::parser::{node_range, run_query};

#[allow(deprecated)]
pub fn collect_document_symbols(tree: &Tree, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    symbols.extend(collect_functions(tree, source));
    symbols.extend(collect_dim_variables(tree, source));
    symbols.extend(collect_labels(tree, source));
    symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    symbols
}

#[allow(deprecated)]
fn collect_functions(tree: &Tree, source: &str) -> Vec<DocumentSymbol> {
    let query = "(def_statement) @def";
    let results = run_query(query, tree.root_node(), source);
    let mut symbols = Vec::new();

    for r in &results {
        // Find the def_statement node to get function_name descendant
        let def_node = match tree.root_node().named_descendant_for_point_range(
            tree_sitter::Point::new(
                r.range.start.line as usize,
                r.range.start.character as usize,
            ),
            tree_sitter::Point::new(
                r.range.start.line as usize,
                r.range.start.character as usize,
            ),
        ) {
            Some(n) => {
                // Walk up to def_statement
                let mut node = n;
                while node.kind() != "def_statement" {
                    match node.parent() {
                        Some(p) => node = p,
                        None => break,
                    }
                }
                if node.kind() == "def_statement" {
                    node
                } else {
                    continue;
                }
            }
            None => continue,
        };

        // DFS for function_name
        let fn_name_node = find_child_by_kind(def_node, "function_name");
        let fn_name_node = match fn_name_node {
            Some(n) => n,
            None => continue,
        };

        let name = match fn_name_node.utf8_text(source.as_bytes()) {
            Ok(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };

        symbols.push(DocumentSymbol {
            name,
            detail: Some("function".to_string()),
            kind: SymbolKind::FUNCTION,
            tags: None,
            deprecated: None,
            range: r.range,
            selection_range: node_range(fn_name_node),
            children: None,
        });
    }

    symbols
}

fn find_child_by_kind<'a>(
    node: tree_sitter::Node<'a>,
    kind: &str,
) -> Option<tree_sitter::Node<'a>> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == kind {
            return Some(n);
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

#[allow(deprecated)]
fn collect_dim_variables(tree: &Tree, source: &str) -> Vec<DocumentSymbol> {
    let queries = [
        (
            "(dim_statement (stringreference name: (_) @name))",
            "string",
        ),
        (
            "(dim_statement (numberreference name: (_) @name))",
            "number",
        ),
        (
            "(dim_statement (stringarray name: (_) @name))",
            "stringarray",
        ),
        (
            "(dim_statement (numberarray name: (_) @name))",
            "numberarray",
        ),
    ];

    let mut symbols = Vec::new();
    for (query_str, detail) in &queries {
        let results = run_query(query_str, tree.root_node(), source);
        for r in &results {
            if r.text.is_empty() {
                continue;
            }
            symbols.push(DocumentSymbol {
                name: r.text.clone(),
                detail: Some(detail.to_string()),
                kind: SymbolKind::VARIABLE,
                tags: None,
                deprecated: None,
                range: r.range,
                selection_range: r.range,
                children: None,
            });
        }
    }

    symbols
}

#[allow(deprecated)]
fn collect_labels(tree: &Tree, source: &str) -> Vec<DocumentSymbol> {
    let query = "((label) @label)";
    let results = run_query(query, tree.root_node(), source);

    results
        .into_iter()
        .filter_map(|r| {
            let name = r.text.trim_end_matches(':').to_string();
            if name.is_empty() {
                return None;
            }
            let selection_range = Range {
                start: r.range.start,
                end: Position {
                    line: r.range.end.line,
                    character: r.range.end.character.saturating_sub(1),
                },
            };
            Some(DocumentSymbol {
                name,
                detail: Some("label".to_string()),
                kind: SymbolKind::NULL,
                tags: None,
                deprecated: None,
                range: r.range,
                selection_range,
                children: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_and_collect(source: &str) -> Vec<DocumentSymbol> {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        collect_document_symbols(&tree, source)
    }

    #[test]
    fn function_symbols() {
        let source = "def fnAdd(A, B) = A + B\ndef fnSub(A, B) = A - B\n";
        let symbols = parse_and_collect(source);
        let funcs: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::FUNCTION)
            .collect();
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].name, "fnAdd");
        assert_eq!(funcs[1].name, "fnSub");
        assert_eq!(funcs[0].detail.as_deref(), Some("function"));
    }

    #[test]
    fn dim_variable_symbols() {
        let source = "dim X$*30, Y, Z$(10)*20\n";
        let symbols = parse_and_collect(source);
        let vars: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::VARIABLE)
            .collect();
        assert!(vars.len() >= 2);
    }

    #[test]
    fn label_symbols() {
        let source = "START:\nlet x = 1\nEND:\n";
        let symbols = parse_and_collect(source);
        let labels: Vec<_> = symbols
            .iter()
            .filter(|s| s.kind == SymbolKind::NULL)
            .collect();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].name, "START");
        assert_eq!(labels[1].name, "END");
        // Selection range should exclude the colon
        assert_eq!(
            labels[0].selection_range.end.character,
            labels[0].range.end.character - 1
        );
    }

    #[test]
    fn sorted_by_position() {
        let source = "ALABEL:\ndim X$*30\ndef fnFoo(A) = A\n";
        let symbols = parse_and_collect(source);
        for i in 1..symbols.len() {
            assert!(
                symbols[i].range.start.line >= symbols[i - 1].range.start.line,
                "Symbols not sorted by position"
            );
        }
    }

    #[test]
    fn empty_source() {
        let symbols = parse_and_collect("");
        assert!(symbols.is_empty());
    }

    #[test]
    fn no_line_numbers_in_symbols() {
        let source = "00100 let x = 1\n00200 let y = 2\n";
        let symbols = parse_and_collect(source);
        // Line numbers should not appear as symbols
        for s in &symbols {
            assert_ne!(s.detail.as_deref(), Some("line_number"));
        }
    }
}
