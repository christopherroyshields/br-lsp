use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Position, Range};
use tree_sitter::{Node, Parser, Point, Query, QueryCursor, StreamingIterator, Tree};

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
    let query = match Query::new(&language, query_str) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
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
}
