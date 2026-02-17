use std::collections::{HashMap, HashSet};

use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use tree_sitter::{Node, Query, QueryCursor, StreamingIterator, Tree};

use crate::workspace::WorkspaceIndex;
use crate::{builtins, extract, extract::ParamKind, parser};

pub fn collect_function_diagnostics(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let mut diagnostics = check_missing_fnend(tree, source);
    diagnostics.extend(check_duplicate_functions(tree, source));
    diagnostics.extend(check_parameter_count(tree, source));
    diagnostics
}

pub fn check_undefined_functions(
    tree: &Tree,
    source: &str,
    index: &WorkspaceIndex,
) -> Vec<Diagnostic> {
    let language = tree.language();
    let query = match Query::new(
        &language,
        "(numeric_user_function) @call
         (string_user_function) @call",
    ) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };

    // Build local names set from definitions in this file
    let local_defs = extract::extract_definitions(tree, source);
    let local_names: HashSet<String> = local_defs
        .iter()
        .map(|d| d.name.to_ascii_lowercase())
        .collect();

    let bytes = source.as_bytes();
    let mut diagnostics = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);

    while let Some(m) = matches.next() {
        let call_node = m.captures[0].node;

        // Extract function_name child
        let name_node = match call_node
            .children(&mut call_node.walk())
            .find(|c| c.kind() == "function_name")
        {
            Some(n) => n,
            None => continue,
        };
        let fn_name = match name_node.utf8_text(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        let key = fn_name.to_ascii_lowercase();

        // Skip if defined locally or in workspace index
        if local_names.contains(&key) || !index.lookup(&key).is_empty() {
            continue;
        }

        diagnostics.push(Diagnostic {
            range: parser::node_range(name_node),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String("undefined-function".to_string())),
            message: format!("Function '{fn_name}' is not defined in the workspace"),
            ..Default::default()
        });
    }

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
                severity: Some(DiagnosticSeverity::WARNING),
                message: format!("Function '{name}' is already defined in this file"),
                ..Default::default()
            });
        } else {
            seen.insert(key.clone(), true);
        }
    }

    diagnostics
}

/// Count argument positions in an `arguments` node.
/// Returns (number of commas) + 1, or 0 if the parens are empty.
fn count_arg_positions(args_node: Node, source: &[u8]) -> usize {
    let mut commas = 0;
    let mut has_argument = false;
    let mut cursor = args_node.walk();
    for child in args_node.children(&mut cursor) {
        if child.kind() == "argument" {
            has_argument = true;
        } else if !child.is_named() && child.utf8_text(source).ok() == Some(",") {
            commas += 1;
        }
    }
    if commas == 0 && !has_argument {
        0
    } else {
        commas + 1
    }
}

/// Count required and total parameters for a builtin function overload.
fn builtin_param_counts(func: &builtins::BuiltinFunction) -> (usize, usize) {
    let required = func
        .params
        .iter()
        .filter(|p| !p.name.starts_with('['))
        .count();
    // [...] as the last param means unlimited (varargs)
    let total = if func.params.last().is_some_and(|p| p.name == "[...]") {
        usize::MAX
    } else {
        func.params.len()
    };
    (required, total)
}

/// Determine the type of an argument node by walking: argument → expression → concrete type.
pub(crate) fn argument_type(arg_node: Node) -> Option<ParamKind> {
    // argument's first named child should be `expression`
    let expr = arg_node.named_child(0)?;
    if expr.kind() != "expression" {
        return None;
    }
    // expression's first named child is the concrete typed expression
    let concrete = expr.named_child(0)?;
    match concrete.kind() {
        "numeric_expression" => Some(ParamKind::Numeric),
        "string_expression" => Some(ParamKind::String),
        "numeric_array_expression" => Some(ParamKind::NumericArray),
        "string_array_expression" => Some(ParamKind::StringArray),
        _ => None,
    }
}

/// Collect argument nodes paired with their positional index from an `arguments` node.
/// Empty positions (e.g. between consecutive commas) yield (index, None).
pub(crate) fn collect_argument_nodes<'a>(args_node: Node<'a>, source: &[u8]) -> Vec<(usize, Option<Node<'a>>)> {
    let mut result = Vec::new();
    let mut pos = 0;
    let mut has_any = false;
    let mut cursor = args_node.walk();
    for child in args_node.children(&mut cursor) {
        if child.kind() == "argument" {
            has_any = true;
            result.push((pos, Some(child)));
        } else if !child.is_named() && child.utf8_text(source).ok() == Some(",") {
            // If the last entry wasn't for this position, record an empty slot
            if result.last().is_none_or(|(p, _)| *p != pos) {
                result.push((pos, None));
            }
            pos += 1;
        }
    }
    // Handle trailing empty position (e.g. `fnFoo(1,)`)
    if pos > 0 && result.last().is_none_or(|(p, _)| *p != pos) {
        result.push((pos, None));
    }
    if !has_any && result.is_empty() {
        return result; // empty parens
    }
    result
}

/// Check if an actual argument type is compatible with the expected parameter type.
/// In BR, a scalar can be passed where an array of the same base type is expected.
fn types_compatible(expected: ParamKind, actual: ParamKind) -> bool {
    if expected == actual {
        return true;
    }
    matches!(
        (expected, actual),
        (ParamKind::NumericArray, ParamKind::Numeric)
            | (ParamKind::StringArray, ParamKind::String)
    )
}

fn format_param_kind(kind: ParamKind) -> &'static str {
    match kind {
        ParamKind::Numeric => "numeric",
        ParamKind::String => "string",
        ParamKind::NumericArray => "numeric array",
        ParamKind::StringArray => "string array",
    }
}

fn check_parameter_count(tree: &Tree, source: &str) -> Vec<Diagnostic> {
    let language = tree.language();
    let query = match Query::new(
        &language,
        "(numeric_user_function) @call
         (string_user_function) @call
         (numeric_system_function) @call
         (string_system_function) @call",
    ) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };

    // Build a map of local function definitions (lowercase name -> def)
    let local_defs = extract::extract_definitions(tree, source);
    let mut def_map: HashMap<String, &extract::FunctionDef> = HashMap::new();
    for def in &local_defs {
        def_map.entry(def.name.to_ascii_lowercase()).or_insert(def);
    }

    let bytes = source.as_bytes();
    let mut diagnostics = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);

    while let Some(m) = matches.next() {
        let call_node = m.captures[0].node;
        let kind = call_node.kind();

        // Extract function name from the function_name child
        let name_node = match call_node
            .children(&mut call_node.walk())
            .find(|c| c.kind() == "function_name")
        {
            Some(n) => n,
            None => continue,
        };
        let fn_name = match name_node.utf8_text(bytes) {
            Ok(s) => s,
            Err(_) => continue,
        };

        // Count argument positions.
        // Some grammar rules (e.g. `udim`) inline their parens instead of using
        // an `arguments` field — skip those rather than assuming 0 args.
        let args_node = call_node.child_by_field_name("arguments");
        let has_paren = args_node.is_some()
            || call_node
                .children(&mut call_node.walk())
                .any(|c| !c.is_named() && c.utf8_text(bytes).ok() == Some("("));
        let arg_count = if let Some(args) = args_node {
            count_arg_positions(args, bytes)
        } else if has_paren {
            // Inline parens (e.g. `udim`) — skip rather than assuming 0 args
            continue;
        } else {
            0
        };

        // System function names (Tab, Rec, etc.) without parentheses are
        // ambiguous — they could be variable references. Skip the check.
        if !has_paren
            && (kind == "numeric_system_function" || kind == "string_system_function")
        {
            continue;
        }

        if kind == "numeric_user_function" || kind == "string_user_function" {
            let key = fn_name.to_ascii_lowercase();
            let def = match def_map.get(&key) {
                Some(d) => d,
                None => continue,
            };
            // Skip checking functions with param substitutions (e.g. [[Name]])
            // or import-only declarations (LIBRARY "path": fnName) since we
            // don't know the actual parameter count/types
            if def.has_param_substitution || def.is_import_only {
                continue;
            }
            let required = def.params.iter().filter(|p| !p.is_optional).count();
            let total = def.params.len();

            if arg_count < required || arg_count > total {
                let expected = if required == total {
                    format!("{required}")
                } else {
                    format!("{required}-{total}")
                };
                diagnostics.push(Diagnostic {
                    range: parser::node_range(call_node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "Function '{fn_name}' expects {expected} parameter(s), but {arg_count} provided"
                    ),
                    ..Default::default()
                });
            } else if let Some(args) = args_node {
                // Count is OK — check types
                let arg_nodes = collect_argument_nodes(args, bytes);
                for (pos, arg_opt) in &arg_nodes {
                    let arg = match arg_opt {
                        Some(a) => *a,
                        None => continue, // empty position — skip
                    };
                    let param = match def.params.get(*pos) {
                        Some(p) => p,
                        None => continue,
                    };
                    let actual = match argument_type(arg) {
                        Some(t) => t,
                        None => continue,
                    };
                    if !types_compatible(param.kind, actual) {
                        diagnostics.push(Diagnostic {
                            range: parser::node_range(arg),
                            severity: Some(DiagnosticSeverity::WARNING),
                            message: format!(
                                "Expected {} argument at position {}, got {}",
                                format_param_kind(param.kind),
                                pos + 1,
                                format_param_kind(actual)
                            ),
                            ..Default::default()
                        });
                    }
                }
            }
        } else {
            // System/builtin function
            let overloads = builtins::lookup(fn_name);
            if overloads.is_empty() {
                continue;
            }

            // Find overloads that match the arg count
            let matching: Vec<&builtins::BuiltinFunction> = overloads
                .iter()
                .filter(|o| {
                    let (req, tot) = builtin_param_counts(o);
                    arg_count >= req && arg_count <= tot
                })
                .collect();

            if matching.is_empty() {
                // No overload matched — emit count diagnostic
                let (req, tot) = builtin_param_counts(&overloads[0]);
                let expected = if req == tot {
                    format!("{req}")
                } else {
                    format!("{req}-{tot}")
                };
                diagnostics.push(Diagnostic {
                    range: parser::node_range(call_node),
                    severity: Some(DiagnosticSeverity::WARNING),
                    message: format!(
                        "Function '{}' expects {expected} parameter(s), but {arg_count} provided",
                        overloads[0].name
                    ),
                    ..Default::default()
                });
            } else if let Some(args) = args_node {
                // Count matched — check types against matching overloads
                let arg_nodes = collect_argument_nodes(args, bytes);
                for (pos, arg_opt) in &arg_nodes {
                    let arg = match arg_opt {
                        Some(a) => *a,
                        None => continue,
                    };
                    let actual = match argument_type(arg) {
                        Some(t) => t,
                        None => continue,
                    };
                    // Only emit if NO matching overload accepts this type at this position
                    let any_accepts = matching.iter().any(|o| {
                        match o.params.get(*pos) {
                            Some(p) => match p.kind() {
                                Some(expected) => types_compatible(expected, actual),
                                None => true, // unknown/literal param — accept anything
                            },
                            None => true, // beyond param list — accept (already count-checked)
                        }
                    });
                    if !any_accepts {
                        let expected_kind = matching[0]
                            .params
                            .get(*pos)
                            .and_then(|p| p.kind());
                        if let Some(expected) = expected_kind {
                            diagnostics.push(Diagnostic {
                                range: parser::node_range(arg),
                                severity: Some(DiagnosticSeverity::WARNING),
                                message: format!(
                                    "Expected {} argument at position {}, got {}",
                                    format_param_kind(expected),
                                    pos + 1,
                                    format_param_kind(actual)
                                ),
                                ..Default::default()
                            });
                        }
                    }
                }
            }
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

    // --- Parameter count tests ---

    #[test]
    fn param_count_too_few() {
        let source = "def fnFoo(A,B)=A+B\nlet X=fnFoo(1)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnFoo"));
        assert!(diags[0].message.contains("2"));
        assert!(diags[0].message.contains("1 provided"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn param_count_too_many() {
        let source = "def fnFoo(A)=A\nlet X=fnFoo(1,2)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnFoo"));
        assert!(diags[0].message.contains("2 provided"));
    }

    #[test]
    fn param_count_correct() {
        let source = "def fnFoo(A,B)=A+B\nlet X=fnFoo(1,2)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn param_count_optional_within_range() {
        let source = "def fnFoo(A;B)\nlet X=fnFoo(1)\nfnend\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "1 arg is within 1-2 range");
    }

    #[test]
    fn param_count_optional_below_required() {
        let source = "def fnFoo(A,B;C)\nlet X=fnFoo(1)\nfnend\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("2-3"));
        assert!(diags[0].message.contains("1 provided"));
    }

    #[test]
    fn param_count_no_parens() {
        let source = "def fnFoo(A)=A\nlet X=fnFoo\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("0 provided"));
    }

    #[test]
    fn param_count_no_params_with_args() {
        let source = "def fnConst=42\nlet X=fnConst(1)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnConst"));
        assert!(diags[0].message.contains("1 provided"));
    }

    #[test]
    fn param_count_builtin_correct() {
        let source = "let X=Val(\"5\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn param_count_builtin_too_many() {
        let source = "let X=Val(\"5\",2)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("Val"));
        assert!(diags[0].message.contains("2 provided"));
    }

    #[test]
    fn param_count_builtin_optional() {
        let source = "let X$=Date$(1)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "Date$ has optional second param");
    }

    #[test]
    fn param_count_builtin_overload() {
        // Decrypt$ has 2 overloads, both with 2 params (1 required + 1 optional, and 2 required)
        // Calling with 1 arg should match the first overload
        let source = "let X$=Decrypt$(\"data\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "should match at least one overload");
    }

    #[test]
    fn param_count_empty_positions() {
        let source = "def fnFoo(A,B)=A+B\nlet X=fnFoo(,)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "(,) counts as 2 positions");
    }

    #[test]
    fn param_count_string_function() {
        let source = "def fnName$(A$)=A$\nlet X$=fnName$(\"hi\",\"extra\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnName$"));
        assert!(diags[0].message.contains("2 provided"));
    }

    #[test]
    fn param_count_udim_no_false_positive() {
        // udim has inline parens in the grammar, not an `arguments` field
        let source = "dim A$(3)*10\nlet X=udim(mat A$)+1\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "udim with inline args should not trigger diagnostic");
    }

    // --- Parameter type tests ---

    #[test]
    fn type_mismatch_string_for_numeric() {
        let source = "def fnFoo(A)=A\nlet X=fnFoo(\"hi\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("numeric"));
        assert!(diags[0].message.contains("string"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn type_mismatch_numeric_for_string() {
        let source = "def fnFoo$(A$)=A$\nlet X$=fnFoo$(42)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("string"));
        assert!(diags[0].message.contains("numeric"));
    }

    #[test]
    fn type_match_correct() {
        let source = "def fnFoo(A, B$)=A\nlet X=fnFoo(1, \"hi\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn type_array_mismatch() {
        let source = "dim B$(3)*10\ndef fnFoo(mat A)\nfnend\nlet X=fnFoo(mat B$)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("numeric array"));
        assert!(diags[0].message.contains("string array"));
    }

    #[test]
    fn type_scalar_vs_array() {
        let source = "dim B(3)\ndef fnFoo(A)=A\nlet X=fnFoo(mat B)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("numeric"));
        assert!(diags[0].message.contains("numeric array"));
    }

    #[test]
    fn type_scalar_for_array_ok() {
        // In BR, a scalar can be passed where an array of the same base type is expected
        let source = "def fnFoo(mat A$)\nfnend\nlet X=fnFoo(\"hi\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "scalar string for string array should be OK");
    }

    #[test]
    fn type_wrong_scalar_for_array() {
        // But a numeric scalar for a string array param is still wrong
        let source = "def fnFoo(mat A$)\nfnend\nlet X=fnFoo(42)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("string array"));
        assert!(diags[0].message.contains("numeric"));
    }

    #[test]
    fn type_empty_position_skip() {
        let source = "def fnFoo(A, B$)=A\nlet X=fnFoo(1,)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn type_builtin_val_correct() {
        let source = "let X=Val(\"123\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn type_builtin_val_wrong() {
        let source = "let X=Val(42)\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("string"));
        assert!(diags[0].message.contains("numeric"));
    }

    #[test]
    fn type_builtin_len_correct() {
        let source = "let X=Len(\"hi\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty());
    }

    #[test]
    fn type_builtin_abs_wrong() {
        let source = "let X=Abs(\"hi\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("numeric"));
        assert!(diags[0].message.contains("string"));
    }

    #[test]
    fn type_builtin_mat2str_string_array() {
        // Mat2Str accepts either numeric or string arrays
        let source = "dim A$(3)*10\nlet mat2str(mat A$,B$,\",\")\n";
        let tree = parse(source);
        let diags = check_parameter_count(&tree, source);
        assert!(diags.is_empty(), "Mat2Str should accept string arrays: {diags:?}");
    }

    #[test]
    #[ignore] // run with: cargo test check_real_file -- --ignored --nocapture
    fn check_real_file() {
        let path = std::env::var("BR_CHECK_FILE")
            .unwrap_or_else(|_| "/home/chris/projects/sra/BR/fileio.brs".to_string());
        let bytes = std::fs::read(&path).expect("failed to read file");
        let source: String = bytes.iter().map(|&b| b as char).collect();

        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, &source, None).expect("parse failed");
        let diags = check_parameter_count(&tree, &source);

        for d in &diags {
            let line = d.range.start.line + 1;
            eprintln!("line {line}: {}", d.message);
        }
        eprintln!("\nTotal: {} parameter count diagnostics", diags.len());
    }

    // --- Undefined function tests ---

    #[test]
    fn undefined_function_warns() {
        let source = "let X=fnFoo(1)\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnFoo"));
        assert!(diags[0].message.contains("not defined"));
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::WARNING));
    }

    #[test]
    fn defined_locally_no_warning() {
        let source = "def fnFoo(X)=X*2\nlet Y=fnFoo(1)\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert!(diags.is_empty(), "locally defined function should not warn");
    }

    #[test]
    fn defined_in_workspace_no_warning() {
        let source = "let X=fnFoo(1)\n";
        let tree = parse(source);

        let mut index = WorkspaceIndex::new();
        let uri = tower_lsp::lsp_types::Url::parse("file:///other.brs").unwrap();
        index.add_file(
            &uri,
            vec![extract::FunctionDef {
                name: "fnFoo".to_string(),
                range: Default::default(),
                selection_range: Default::default(),
                is_library: false,
                is_import_only: false,
                params: vec![],
                has_param_substitution: false,
                documentation: None,
                return_documentation: None,
            }],
        );

        let diags = check_undefined_functions(&tree, source, &index);
        assert!(diags.is_empty(), "workspace-defined function should not warn");
    }

    #[test]
    fn undefined_case_insensitive() {
        let source = "def fnfoo(X)=X\nlet Y=FNFOO(1)\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert!(diags.is_empty(), "case-insensitive match should not warn");
    }

    #[test]
    fn undefined_string_function() {
        let source = "let X$=fnName$(\"hi\")\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("fnName$"));
        assert!(diags[0].message.contains("not defined"));
    }

    #[test]
    fn library_import_not_flagged() {
        let source = "library \"rtflib.dll\": fnRTF\nlet X=fnRTF(1,2,3)\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert!(
            diags.is_empty(),
            "LIBRARY-imported function should not warn: {diags:?}"
        );
    }

    #[test]
    fn system_function_not_flagged() {
        let source = "let X=Val(\"5\")\n";
        let tree = parse(source);
        let index = WorkspaceIndex::new();
        let diags = check_undefined_functions(&tree, source, &index);
        assert!(diags.is_empty(), "system functions should not be checked");
    }
}
