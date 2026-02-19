use std::collections::HashMap;

use tower_lsp::lsp_types::*;
use tree_sitter::{Node, Tree};

use crate::diagnostics;
use crate::extract::ParamKind;
use crate::parser;

/// If the diagnostic is an undefined-function warning, generate a code action
/// that inserts a function stub at the end of the file.
pub fn create_function_stub_action(
    uri: &Url,
    diagnostic: &Diagnostic,
    tree: &Tree,
    source: &str,
) -> Option<CodeAction> {
    // Only handle our undefined-function diagnostic
    match &diagnostic.code {
        Some(NumberOrString::String(code)) if code == "undefined-function" => {}
        _ => return None,
    }

    let fn_name = extract_function_name(&diagnostic.message)?;

    // Find the call-site node to inspect arguments
    let call_node = find_call_node(tree, source, diagnostic.range.start)?;

    // Collect argument info
    let params = infer_params(&call_node, source);

    // Find the last line number in the file
    let last_ln = last_line_number(tree, source);
    let stub_start = next_line_number(last_ln);

    // Generate the stub text
    let stub = generate_stub(&fn_name, &params, stub_start);

    // Insert at the end of the document
    let line_count = source.lines().count() as u32;
    let insert_pos = Position {
        line: line_count,
        character: 0,
    };

    let text_edit = TextEdit {
        range: Range {
            start: insert_pos,
            end: insert_pos,
        },
        new_text: stub,
    };

    let mut changes = HashMap::new();
    changes.insert(uri.clone(), vec![text_edit]);

    Some(CodeAction {
        title: format!("Generate function stub for '{fn_name}'"),
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        }),
        ..Default::default()
    })
}

/// Extract the function name from the diagnostic message.
/// Message format: "Function 'fnName' is not defined in the workspace"
fn extract_function_name(message: &str) -> Option<String> {
    let start = message.find('\'')?;
    let end = message[start + 1..].find('\'')?;
    Some(message[start + 1..start + 1 + end].to_string())
}

/// Find the call-site node (numeric_user_function or string_user_function)
/// at the diagnostic position.
fn find_call_node<'a>(tree: &'a Tree, _source: &str, pos: Position) -> Option<Node<'a>> {
    let node = parser::node_at_position(tree, pos.line as usize, pos.character as usize)?;

    // Walk up to find the user function call node
    let mut current = node;
    loop {
        match current.kind() {
            "numeric_user_function" | "string_user_function" => return Some(current),
            _ => current = current.parent()?,
        }
    }
}

struct ParamInfo {
    name: String,
    kind: ParamKind,
}

/// Infer parameter names and types from the call-site arguments.
fn infer_params(call_node: &Node, source: &str) -> Vec<ParamInfo> {
    let args_node = match call_node.child_by_field_name("arguments") {
        Some(n) => n,
        None => return Vec::new(),
    };

    let bytes = source.as_bytes();
    let arg_nodes = diagnostics::collect_argument_nodes(args_node, bytes);

    arg_nodes
        .iter()
        .enumerate()
        .map(|(i, (_, arg_opt))| {
            let kind = arg_opt
                .and_then(|n| diagnostics::argument_type(n))
                .unwrap_or(ParamKind::Numeric);

            let name = arg_opt
                .and_then(|n| infer_param_name(n, bytes, i, kind))
                .unwrap_or_else(|| generic_param_name(i, kind));

            ParamInfo { name, kind }
        })
        .collect()
}

/// Try to infer a parameter name from a simple variable reference argument.
fn infer_param_name(
    arg_node: Node,
    source: &[u8],
    _index: usize,
    _kind: ParamKind,
) -> Option<String> {
    // Walk: argument → expression → typed_expression → identifier/reference
    let expr = arg_node.named_child(0)?;
    if expr.kind() != "expression" {
        return None;
    }
    let typed_expr = expr.named_child(0)?;
    let var_node = find_variable_node(typed_expr)?;

    let var_text = var_node.utf8_text(source).ok()?;

    match var_node.kind() {
        "numberarray" | "stringarray" => {
            // Array reference — find the identifier child for the name
            let id_node = find_identifier_in_array(var_node)?;
            let id_text = id_node.utf8_text(source).ok()?;
            Some(format!("Mat {id_text}"))
        }
        "numberidentifier" | "stringidentifier" | "stringreference" => {
            // Simple variable — use its name directly
            Some(var_text.to_string())
        }
        _ => None,
    }
}

/// Recursively walk down into a typed expression to find a variable reference node.
fn find_variable_node(node: Node) -> Option<Node> {
    const VAR_KINDS: &[&str] = &[
        "numberidentifier",
        "stringidentifier",
        "numberarray",
        "stringarray",
        "stringreference",
    ];

    if VAR_KINDS.contains(&node.kind()) {
        return Some(node);
    }

    // Only descend if the node has exactly one named child (simple expression path)
    // This avoids matching inside complex expressions like `A + B`
    let named_count = node.named_child_count();
    if named_count == 1 {
        return find_variable_node(node.named_child(0)?);
    }

    None
}

/// Find the identifier child inside an array node (numberarray/stringarray).
fn find_identifier_in_array(array_node: Node) -> Option<Node> {
    let mut cursor = array_node.walk();
    for child in array_node.children(&mut cursor) {
        match child.kind() {
            "numberidentifier" | "stringidentifier" => return Some(child),
            _ => {}
        }
    }
    None
}

/// Generate a generic parameter name for a given position and type.
fn generic_param_name(index: usize, kind: ParamKind) -> String {
    match kind {
        ParamKind::Numeric => format!("Param{}", index + 1),
        ParamKind::String => format!("Param{}$", index + 1),
        ParamKind::NumericArray => format!("Mat Param{}", index + 1),
        ParamKind::StringArray => format!("Mat Param{}$", index + 1),
    }
}

/// Format a parameter for the DEF line.
fn format_param(param: &ParamInfo) -> String {
    if param.name.starts_with("Mat ") {
        // Already has Mat prefix from inference
        param.name.clone()
    } else {
        match param.kind {
            ParamKind::NumericArray => format!("Mat {}", param.name),
            ParamKind::StringArray => format!("Mat {}", param.name),
            _ => param.name.clone(),
        }
    }
}

/// Find the highest line number in the document.
fn last_line_number(tree: &Tree, source: &str) -> i64 {
    let results = parser::run_query("(line_number) @ln", tree.root_node(), source);
    results
        .iter()
        .filter_map(|r| r.text.trim().parse::<i64>().ok())
        .max()
        .unwrap_or(0)
}

/// Compute the next line number, rounding up to the nearest 10.
fn next_line_number(last: i64) -> i64 {
    ((last / 10) + 1) * 10
}

/// Generate the function stub text.
fn generate_stub(fn_name: &str, params: &[ParamInfo], start_ln: i64) -> String {
    let is_string = fn_name.ends_with('$');
    let default_value = if is_string { "\"\"" } else { "0" };

    let params_str = if params.is_empty() {
        String::new()
    } else {
        let param_list: String = params
            .iter()
            .map(format_param)
            .collect::<Vec<_>>()
            .join(",");
        format!("({param_list})")
    };

    let ln1 = start_ln;
    let ln2 = start_ln + 10;
    let ln3 = start_ln + 20;
    let ln4 = start_ln + 30;

    format!(
        "\n{ln1:05} DEF {fn_name}{params_str}\n{ln2:05} ! TODO: Implement {fn_name}\n{ln3:05} LET {fn_name}={default_value}\n{ln4:05} FNEND\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse(source: &str) -> Tree {
        let mut p = parser::new_parser();
        parser::parse(&mut p, source, None).expect("parse failed")
    }

    fn make_undefined_diagnostic(range: Range, fn_name: &str) -> Diagnostic {
        Diagnostic {
            range,
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String("undefined-function".to_string())),
            message: format!("Function '{fn_name}' is not defined in the workspace"),
            ..Default::default()
        }
    }

    #[test]
    fn numeric_function_stub() {
        let source = "00010 let X = fnFoo(A, B)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        // Find the function name range
        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnFoo").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnFoo");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        assert!(action.title.contains("fnFoo"));

        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        assert!(new_text.contains("DEF fnFoo(A,B)"));
        assert!(new_text.contains("LET fnFoo=0"));
        assert!(new_text.contains("FNEND"));
        assert!(new_text.contains("TODO"));
    }

    #[test]
    fn string_function_stub() {
        let source = "00010 let X$ = fnBar$(Name$)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnBar$").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnBar$");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        assert!(new_text.contains("DEF fnBar$(Name$)"));
        assert!(
            new_text.contains("LET fnBar$=\"\""),
            "string function should default to empty string, got: {new_text}"
        );
        assert!(new_text.contains("FNEND"));
    }

    #[test]
    fn mixed_param_types() {
        let source = "00010 dim Items$(5)*30\n00020 let X = fnCalc(Count, Name$, mat Items$)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnCalc").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnCalc");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        assert!(
            new_text.contains("Count"),
            "should use variable name Count: {new_text}"
        );
        assert!(
            new_text.contains("Name$"),
            "should use variable name Name$: {new_text}"
        );
        assert!(
            new_text.contains("Mat Items$"),
            "should use Mat array name: {new_text}"
        );
    }

    #[test]
    fn expression_args_use_generic_names() {
        let source = "00010 let X = fnFoo(1+2, \"hello\")\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnFoo").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnFoo");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        assert!(
            new_text.contains("Param1"),
            "expression arg should get generic name: {new_text}"
        );
        assert!(
            new_text.contains("Param2$"),
            "string expression arg should get $ suffix: {new_text}"
        );
    }

    #[test]
    fn no_action_for_wrong_diagnostic_code() {
        let source = "00010 let X = fnFoo(1)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let diag = Diagnostic {
            range: Range::default(),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String("some-other-code".to_string())),
            message: "Something else".to_string(),
            ..Default::default()
        };

        let action = create_function_stub_action(&uri, &diag, &tree, source);
        assert!(action.is_none());
    }

    #[test]
    fn no_action_for_no_diagnostic_code() {
        let source = "00010 let X = fnFoo(1)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let diag = Diagnostic {
            range: Range::default(),
            severity: Some(DiagnosticSeverity::WARNING),
            message: "Something else".to_string(),
            ..Default::default()
        };

        let action = create_function_stub_action(&uri, &diag, &tree, source);
        assert!(action.is_none());
    }

    #[test]
    fn line_number_calculation() {
        let source = "00010 let X = 1\n00020 let Y = 2\n00100 let Z = 3\n";
        let tree = parse(source);
        let ln = last_line_number(&tree, source);
        assert_eq!(ln, 100);
        assert_eq!(next_line_number(ln), 110);
    }

    #[test]
    fn line_number_no_lines() {
        let source = "let X = 1\n";
        let tree = parse(source);
        let ln = last_line_number(&tree, source);
        assert_eq!(ln, 0);
        assert_eq!(next_line_number(ln), 10);
    }

    #[test]
    fn line_number_in_stub() {
        let source = "00100 let X = fnFoo(A)\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnFoo").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnFoo");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        // Last line is 100, so stub should start at 110
        assert!(
            new_text.contains("00110 DEF"),
            "stub should start at 00110: {new_text}"
        );
        assert!(new_text.contains("00120 !"), "comment at 00120: {new_text}");
        assert!(new_text.contains("00130 LET"), "let at 00130: {new_text}");
        assert!(
            new_text.contains("00140 FNEND"),
            "fnend at 00140: {new_text}"
        );
    }

    #[test]
    fn no_params_omits_parentheses() {
        let source = "00010 let X = fnConst\n";
        let tree = parse(source);
        let uri = Url::parse("file:///test.brs").unwrap();

        let results = parser::run_query("(function_name) @fn", tree.root_node(), source);
        let fn_result = results.iter().find(|r| r.text == "fnConst").unwrap();
        let diag = make_undefined_diagnostic(fn_result.range, "fnConst");

        let action = create_function_stub_action(&uri, &diag, &tree, source).unwrap();
        let edit = action.edit.unwrap();
        let changes = edit.changes.unwrap();
        let edits = changes.get(&uri).unwrap();
        let new_text = &edits[0].new_text;

        assert!(
            new_text.contains("DEF fnConst\n"),
            "no-param function should omit parens: {new_text}"
        );
        assert!(
            !new_text.contains("DEF fnConst("),
            "should not have parentheses: {new_text}"
        );
    }

    #[test]
    fn extract_function_name_from_message() {
        assert_eq!(
            extract_function_name("Function 'fnFoo' is not defined in the workspace"),
            Some("fnFoo".to_string())
        );
        assert_eq!(
            extract_function_name("Function 'fnBar$' is not defined in the workspace"),
            Some("fnBar$".to_string())
        );
    }
}
