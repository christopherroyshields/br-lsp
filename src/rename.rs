use tower_lsp::lsp_types::{Position, Range, TextEdit};
use tree_sitter::Tree;

use crate::builtins;
use crate::parser::{node_at_position, node_range};
use crate::references;

const SUPPORTED_KINDS: &[&str] = &[
    "function_name",
    "label",
    "label_reference",
    "stringidentifier",
    "numberidentifier",
];

pub struct PrepareRenameResult {
    pub range: Range,
    pub placeholder: String,
}

fn resolve_node<'a>(tree: &'a Tree, _source: &str, line: usize, character: usize) -> Option<tree_sitter::Node<'a>> {
    let mut node = node_at_position(tree, line, character)?;

    // End-of-token fallback (same as find_references)
    if !SUPPORTED_KINDS.contains(&node.kind()) && character > 0 {
        if let Some(n) = node_at_position(tree, line, character - 1) {
            if SUPPORTED_KINDS.contains(&n.kind()) {
                node = n;
            }
        }
    }

    if SUPPORTED_KINDS.contains(&node.kind()) {
        Some(node)
    } else {
        None
    }
}

pub fn prepare_rename(
    tree: &Tree,
    source: &str,
    line: usize,
    character: usize,
) -> Option<PrepareRenameResult> {
    let node = resolve_node(tree, source, line, character)?;
    let text = node.utf8_text(source.as_bytes()).ok()?;

    match node.kind() {
        "function_name" => {
            // Reject system functions
            if !builtins::lookup(text).is_empty() {
                return None;
            }
            Some(PrepareRenameResult {
                range: node_range(node),
                placeholder: text.to_string(),
            })
        }
        "stringidentifier" | "numberidentifier" => Some(PrepareRenameResult {
            range: node_range(node),
            placeholder: text.to_string(),
        }),
        "label" => {
            // Exclude trailing `:` from range and placeholder
            let name = text.trim_end_matches(':');
            let range = node_range(node);
            Some(PrepareRenameResult {
                range: Range {
                    start: range.start,
                    end: Position {
                        line: range.end.line,
                        character: range.end.character.saturating_sub(1),
                    },
                },
                placeholder: name.to_string(),
            })
        }
        "label_reference" => Some(PrepareRenameResult {
            range: node_range(node),
            placeholder: text.to_string(),
        }),
        _ => None,
    }
}

pub fn compute_renames(
    tree: &Tree,
    source: &str,
    line: usize,
    character: usize,
    new_name: &str,
) -> Vec<TextEdit> {
    let node = match resolve_node(tree, source, line, character) {
        Some(n) => n,
        None => return Vec::new(),
    };

    let text = match node.utf8_text(source.as_bytes()) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let ranges = match node.kind() {
        "function_name" => {
            if !builtins::lookup(text).is_empty() {
                return Vec::new();
            }
            references::find_function_refs(&node, tree, source)
        }
        "label" | "label_reference" => references::find_label_refs(&node, tree, source),
        "stringidentifier" | "numberidentifier" => {
            references::find_variable_refs(&node, tree, source)
        }
        _ => return Vec::new(),
    };

    ranges
        .into_iter()
        .map(|range| TextEdit {
            range,
            new_text: new_name.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(source: &str) -> Tree {
        let mut p = parser::new_parser();
        parser::parse(&mut p, source, None).unwrap()
    }

    #[test]
    fn rename_variable() {
        let source = "let X = 1\nprint X\n";
        let tree = parse(source);
        let edits = compute_renames(&tree, source, 0, 4, "Y");
        assert_eq!(edits.len(), 2);
        for edit in &edits {
            assert_eq!(edit.new_text, "Y");
        }
    }

    #[test]
    fn rename_function() {
        let source = "def fnTest(x)\nlet y = fnTest(1)\nfnend\n";
        let tree = parse(source);
        let edits = compute_renames(&tree, source, 0, 4, "fnNew");
        assert_eq!(edits.len(), 2);
        for edit in &edits {
            assert_eq!(edit.new_text, "fnNew");
        }
    }

    #[test]
    fn rename_label() {
        let source = "MYLOOP:\nlet x = 1\ngoto MYLOOP\n";
        let tree = parse(source);
        // Cursor on label definition
        let edits = compute_renames(&tree, source, 0, 0, "NEWLOOP");
        assert_eq!(edits.len(), 2);
        for edit in &edits {
            assert_eq!(edit.new_text, "NEWLOOP");
        }
        // The label definition edit should NOT include the colon — the colon stays
        let def_edit = &edits[0];
        let end_col = def_edit.range.end.character;
        let start_col = def_edit.range.start.character;
        // "MYLOOP" is 6 chars, colon excluded
        assert_eq!(end_col - start_col, 6);
    }

    #[test]
    fn rename_label_from_reference() {
        let source = "MYLOOP:\nlet x = 1\ngoto MYLOOP\n";
        let tree = parse(source);
        // Cursor on label reference (line 2, col 5 = inside "MYLOOP")
        let edits = compute_renames(&tree, source, 2, 5, "NEWLOOP");
        assert_eq!(edits.len(), 2);
    }

    #[test]
    fn reject_system_function() {
        let source = "let x = val(\"123\")\n";
        let tree = parse(source);
        // "val" is at col 8
        let result = prepare_rename(&tree, source, 0, 9);
        assert!(result.is_none());
    }

    #[test]
    fn reject_line_number() {
        let source = "00100 let x = 1\n00200 goto 100\n";
        let tree = parse(source);
        let result = prepare_rename(&tree, source, 0, 2);
        assert!(result.is_none());
    }

    #[test]
    fn prepare_rename_user_function() {
        let source = "def fnTest(x)\nlet y = fnTest(1)\nfnend\n";
        let tree = parse(source);
        let result = prepare_rename(&tree, source, 0, 4).unwrap();
        assert_eq!(result.placeholder, "fnTest");
    }

    #[test]
    fn prepare_rename_label_excludes_colon() {
        let source = "MYLOOP:\nlet x = 1\n";
        let tree = parse(source);
        let result = prepare_rename(&tree, source, 0, 0).unwrap();
        assert_eq!(result.placeholder, "MYLOOP");
        // Range should be 6 chars wide (no colon)
        assert_eq!(
            result.range.end.character - result.range.start.character,
            6
        );
    }

    #[test]
    fn scope_aware_variable_rename() {
        let source = "\
let X = 1
def fnFoo(X)
let Y = X + 1
fnend
let Z = X + 2
";
        let tree = parse(source);
        // Rename X inside function (parameter scope) — line 2
        let x_col = source.lines().nth(2).unwrap().find('X').unwrap();
        let edits = compute_renames(&tree, source, 2, x_col, "A");
        // Should only rename param X and body X (2 refs)
        assert_eq!(edits.len(), 2);

        // Rename X outside function — line 0
        let x_col = source.lines().next().unwrap().find('X').unwrap();
        let edits = compute_renames(&tree, source, 0, x_col, "B");
        // Should only rename module-level X refs (line 0 and line 4)
        assert_eq!(edits.len(), 2);
    }
}
