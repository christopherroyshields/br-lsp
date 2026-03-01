use tower_lsp::lsp_types::{DocumentSymbol, Position, Range, SymbolKind};
use tree_sitter::{Node, Tree, TreeCursor};

use crate::parser::node_range;

#[allow(deprecated)]
pub fn collect_document_symbols(tree: &Tree, source: &str) -> Vec<DocumentSymbol> {
    let mut symbols = Vec::new();
    let mut cursor = tree.walk();
    walk_symbols(&mut cursor, source, &mut symbols);
    symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    symbols
}

#[allow(deprecated)]
fn walk_symbols(cursor: &mut TreeCursor, source: &str, symbols: &mut Vec<DocumentSymbol>) {
    loop {
        let node = cursor.node();
        match node.kind() {
            "def_statement" => {
                if let Some(sym) = make_function_symbol(node, source) {
                    symbols.push(sym);
                }
                // Skip children — we already extracted what we need
                if !cursor.goto_next_sibling() {
                    return;
                }
                continue;
            }
            "dim_statement" => {
                collect_dim_vars(node, source, symbols);
                // Skip children
                if !cursor.goto_next_sibling() {
                    return;
                }
                continue;
            }
            "label" => {
                if let Some(sym) = make_label_symbol(node, source) {
                    symbols.push(sym);
                }
                if !cursor.goto_next_sibling() {
                    return;
                }
                continue;
            }
            _ => {}
        }

        // Recurse into children
        if cursor.goto_first_child() {
            walk_symbols(cursor, source, symbols);
            cursor.goto_parent();
        }

        if !cursor.goto_next_sibling() {
            return;
        }
    }
}

#[allow(deprecated)]
fn make_function_symbol(node: Node, source: &str) -> Option<DocumentSymbol> {
    let fn_name_node = find_child_by_kind(node, "function_name")?;
    let name = fn_name_node.utf8_text(source.as_bytes()).ok()?;
    if name.is_empty() {
        return None;
    }
    Some(DocumentSymbol {
        name: name.to_string(),
        detail: Some("function".to_string()),
        kind: SymbolKind::FUNCTION,
        tags: None,
        deprecated: None,
        range: node_range(node),
        selection_range: node_range(fn_name_node),
        children: None,
    })
}

#[allow(deprecated)]
fn collect_dim_vars(node: Node, source: &str, symbols: &mut Vec<DocumentSymbol>) {
    let mut child_cursor = node.walk();
    for child in node.children(&mut child_cursor) {
        let detail = match child.kind() {
            "stringreference" => "string",
            "numberreference" => "number",
            "stringarray" => "stringarray",
            "numberarray" => "numberarray",
            _ => continue,
        };
        let name_node = match child.child_by_field_name("name") {
            Some(n) => n,
            None => continue,
        };
        let name = match name_node.utf8_text(source.as_bytes()) {
            Ok(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let range = node_range(name_node);
        symbols.push(DocumentSymbol {
            name,
            detail: Some(detail.to_string()),
            kind: SymbolKind::VARIABLE,
            tags: None,
            deprecated: None,
            range,
            selection_range: range,
            children: None,
        });
    }
}

#[allow(deprecated)]
fn make_label_symbol(node: Node, source: &str) -> Option<DocumentSymbol> {
    let text = node.utf8_text(source.as_bytes()).ok()?;
    let name = text.trim_end_matches(':').to_string();
    if name.is_empty() {
        return None;
    }
    let range = node_range(node);
    let selection_range = Range {
        start: range.start,
        end: Position {
            line: range.end.line,
            character: range.end.character.saturating_sub(1),
        },
    };
    Some(DocumentSymbol {
        name,
        detail: Some("label".to_string()),
        kind: SymbolKind::NULL,
        tags: None,
        deprecated: None,
        range,
        selection_range,
        children: None,
    })
}

fn find_child_by_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
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
