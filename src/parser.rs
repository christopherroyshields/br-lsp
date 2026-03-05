use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use tree_sitter::{Language, Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

static QUERY_CACHE: LazyLock<Mutex<HashMap<String, Arc<Query>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn new_parser() -> Parser {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_br::LANGUAGE.into())
        .expect("failed to load BR grammar");
    parser
}

pub fn parse(parser: &mut Parser, source: &str, old_tree: Option<&Tree>) -> Option<Tree> {
    parser.parse(source, old_tree)
}

pub fn node_at_position(tree: &Tree, row: usize, col: usize) -> Option<Node> {
    let point = Point::new(row, col);
    tree.root_node()
        .named_descendant_for_point_range(point, point)
}

pub struct QueryResult {
    pub kind: String,
    pub range: Range,
    pub text: String,
    pub start_byte: usize,
}

pub fn run_query(query_str: &str, node: Node, source: &str) -> Vec<QueryResult> {
    let language = node.language();

    // Try to reuse a cached compiled query
    let cached = {
        let cache = QUERY_CACHE.lock().unwrap();
        cache.get(query_str).cloned()
    };
    let query = match cached {
        Some(q) => q,
        None => {
            let q = match Query::new(&language, query_str) {
                Ok(q) => Arc::new(q),
                Err(_) => return Vec::new(),
            };
            let mut cache = QUERY_CACHE.lock().unwrap();
            Arc::clone(cache.entry(query_str.to_string()).or_insert(q))
        }
    };

    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, node, source.as_bytes());
    let mut results = Vec::new();
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let n = capture.node;
            results.push(QueryResult {
                kind: n.kind().to_string(),
                range: node_range(n),
                text: n.utf8_text(source.as_bytes()).unwrap_or("").to_string(),
                start_byte: n.start_byte(),
            });
        }
    }
    results
}

pub struct CallContext {
    pub name: String,
    pub active_param: u32,
}

/// Text-based fallback for finding function call context when tree-sitter
/// produces ERROR nodes (e.g. unbalanced parentheses while typing).
///
/// Scans backward from the cursor position to find the unmatched opening `(`
/// and extracts the function name preceding it.
pub fn find_function_call_context(source: &str, row: usize, col: usize) -> Option<CallContext> {
    // Convert (row, col) to byte offset
    let offset = source
        .lines()
        .take(row)
        .map(|line| line.len() + 1) // +1 for newline
        .sum::<usize>()
        + col;

    if offset > source.len() {
        return None;
    }

    let bytes = source.as_bytes();
    let mut depth: i32 = 0;
    let mut comma_count: u32 = 0;
    let mut in_string = false;
    let mut i = offset;

    // Scan backward from cursor
    while i > 0 {
        i -= 1;
        let ch = bytes[i] as char;

        if in_string {
            if ch == '"' {
                // Check for BR escaped quote ""
                if i > 0 && bytes[i - 1] == b'"' {
                    i -= 1; // skip the escaped quote
                } else {
                    in_string = false;
                }
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            ')' => depth += 1,
            '(' => {
                depth -= 1;
                if depth < 0 {
                    // Found the unmatched opening paren — extract function name
                    let name_end = i;
                    let mut name_start = name_end;
                    while name_start > 0 {
                        let c = bytes[name_start - 1] as char;
                        if c.is_ascii_alphanumeric() || c == '_' || c == '$' {
                            name_start -= 1;
                        } else {
                            break;
                        }
                    }
                    if name_start == name_end {
                        return None; // no identifier before `(`
                    }
                    let name = String::from_utf8_lossy(&bytes[name_start..name_end]).to_string();
                    return Some(CallContext {
                        name,
                        active_param: comma_count,
                    });
                }
            }
            ',' if depth == 0 => comma_count += 1,
            _ => {}
        }
    }

    None
}

pub struct DiagnosticNodes<'tree> {
    pub def_statements: Vec<Node<'tree>>,
    pub fnend_statements: Vec<Node<'tree>>,
    pub end_def_statements: Vec<Node<'tree>>,
    pub library_statements: Vec<Node<'tree>>,
    pub function_calls: Vec<Node<'tree>>,
    pub function_names: Vec<Node<'tree>>,
    pub var_ref_names: Vec<Node<'tree>>,
    pub dim_var_ref_names: Vec<Node<'tree>>,
    pub param_ident_names: Vec<Node<'tree>>,
}

static DIAGNOSTIC_QUERY: LazyLock<Query> = LazyLock::new(|| {
    let language: Language = tree_sitter_br::LANGUAGE.into();
    Query::new(
        &language,
        "(def_statement) @node
         (fnend_statement) @node
         (end_def_statement) @node
         (library_statement) @node
         (numeric_user_function) @node
         (string_user_function) @node
         (numeric_system_function) @node
         (string_system_function) @node
         (function_name) @node
         (stringreference name: (_) @node)
         (numberreference name: (_) @node)
         (stringarray name: (_) @node)
         (numberarray name: (_) @node)
         (dim_statement (stringreference name: (_) @node))
         (dim_statement (numberreference name: (_) @node))
         (dim_statement (stringarray name: (_) @node))
         (dim_statement (numberarray name: (_) @node))
         (parameter (numeric_parameter (numberreference name: (numberidentifier) @node)))
         (parameter (string_parameter (stringreference name: (stringidentifier) @node)))
         (parameter (number_array_parameter (numberarray name: (numberidentifier) @node)))
         (parameter (string_array_parameter (stringarray name: (stringidentifier) @node)))",
    )
    .expect("failed to compile diagnostic query")
});

pub fn collect_diagnostic_nodes<'tree>(tree: &'tree Tree, source: &str) -> DiagnosticNodes<'tree> {
    let mut nodes = DiagnosticNodes {
        def_statements: Vec::new(),
        fnend_statements: Vec::new(),
        end_def_statements: Vec::new(),
        library_statements: Vec::new(),
        function_calls: Vec::new(),
        function_names: Vec::new(),
        var_ref_names: Vec::new(),
        dim_var_ref_names: Vec::new(),
        param_ident_names: Vec::new(),
    };

    let query = &*DIAGNOSTIC_QUERY;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), source.as_bytes());

    while let Some(m) = matches.next() {
        let node = m.captures[0].node;
        match m.pattern_index {
            0 => nodes.def_statements.push(node),
            1 => nodes.fnend_statements.push(node),
            2 => nodes.end_def_statements.push(node),
            3 => nodes.library_statements.push(node),
            4..=7 => nodes.function_calls.push(node),
            8 => nodes.function_names.push(node),
            9..=12 => nodes.var_ref_names.push(node),
            13..=16 => nodes.dim_var_ref_names.push(node),
            17..=20 => nodes.param_ident_names.push(node),
            _ => {}
        }
    }

    nodes
}

pub fn collect_diagnostics(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    collect_errors(tree.root_node(), source, &mut diagnostics);
    diagnostics
}

fn collect_errors(node: Node, source: &str, diagnostics: &mut Vec<Diagnostic>) {
    if node.is_error() {
        let text = node
            .utf8_text(source.as_bytes())
            .unwrap_or("")
            .chars()
            .take(50)
            .collect::<String>();
        diagnostics.push(Diagnostic {
            range: node_range(node),
            severity: Some(DiagnosticSeverity::ERROR),
            message: format!("Syntax error: unexpected `{text}`"),
            ..Default::default()
        });
        return;
    }

    if node.is_missing() {
        let kind = node.kind();
        diagnostics.push(Diagnostic {
            range: node_range(node),
            severity: Some(DiagnosticSeverity::ERROR),
            message: format!("Syntax error: missing `{kind}`"),
            ..Default::default()
        });
        return;
    }

    if !node.has_error() {
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_errors(child, source, diagnostics);
    }
}

pub fn node_range(node: Node) -> Range {
    let start = node.start_position();
    let end = node.end_position();
    Range {
        start: Position {
            line: start.row as u32,
            character: start.column as u32,
        },
        end: Position {
            line: end.row as u32,
            character: end.column as u32,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_parse_no_errors() {
        let mut parser = new_parser();
        let tree = parse(&mut parser, "let x = 1\n", None).unwrap();
        assert!(!tree.root_node().has_error());
        let diags = collect_diagnostics(&tree, "let x = 1\n");
        assert!(diags.is_empty());
    }

    #[test]
    fn parse_error_produces_diagnostic() {
        let mut parser = new_parser();
        let source = "let x = = =\n";
        let tree = parse(&mut parser, source, None).unwrap();
        assert!(tree.root_node().has_error());
        let diags = collect_diagnostics(&tree, source);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn empty_source() {
        let mut parser = new_parser();
        let tree = parse(&mut parser, "", None).unwrap();
        let diags = collect_diagnostics(&tree, "");
        assert!(diags.is_empty());
    }

    #[test]
    fn call_context_simple() {
        let source = "let x = Val(\"hi\"";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "Val");
        assert_eq!(ctx.active_param, 0);
    }

    #[test]
    fn call_context_multi_arg() {
        let source = "let x = fnFoo(A, B, ";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "fnFoo");
        assert_eq!(ctx.active_param, 2);
    }

    #[test]
    fn call_context_nested() {
        let source = "let x = Val(Str$(X), ";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "Val");
        assert_eq!(ctx.active_param, 1);
    }

    #[test]
    fn call_context_string_with_parens() {
        let source = "let x = fnFoo(\"(hi)\", ";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "fnFoo");
        assert_eq!(ctx.active_param, 1);
    }

    #[test]
    fn call_context_no_args_yet() {
        let source = "let x = Val(";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "Val");
        assert_eq!(ctx.active_param, 0);
    }

    #[test]
    fn call_context_br_escaped_quotes() {
        let source = "let x = fnFoo(\"say \"\"hi\"\"\", ";
        let ctx = find_function_call_context(source, 0, source.len()).unwrap();
        assert_eq!(ctx.name, "fnFoo");
        assert_eq!(ctx.active_param, 1);
    }

    #[test]
    fn call_context_no_function_name() {
        let source = "(";
        assert!(find_function_call_context(source, 0, source.len()).is_none());
    }

    #[test]
    fn call_context_no_open_paren() {
        let source = "let x = 1 + 2";
        assert!(find_function_call_context(source, 0, source.len()).is_none());
    }

    #[test]
    fn call_context_multiline() {
        let source = "let x = fnFoo(A,\nB, ";
        let ctx = find_function_call_context(source, 1, 3).unwrap();
        assert_eq!(ctx.name, "fnFoo");
        assert_eq!(ctx.active_param, 2);
    }
}
