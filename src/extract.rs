use tower_lsp::lsp_types::Range;
use tree_sitter::{Node, Tree};

use crate::parser::node_range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: String,
    pub range: Range,
    pub selection_range: Range,
    pub is_library: bool,
    pub params: Vec<ParamInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamInfo {
    pub name: String,
    pub kind: ParamKind,
    pub is_optional: bool,
    pub is_reference: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Numeric,
    String,
    NumericArray,
    StringArray,
}

pub fn extract_definitions(tree: &Tree, source: &str) -> Vec<FunctionDef> {
    let mut defs = Vec::new();
    let root = tree.root_node();
    collect_def_statements(root, source, &mut defs);
    defs
}

fn collect_def_statements(node: Node, source: &str, defs: &mut Vec<FunctionDef>) {
    if node.kind() == "def_statement" {
        if let Some(def) = extract_one_def(node, source) {
            defs.push(def);
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_def_statements(child, source, defs);
    }
}

fn extract_one_def(def_node: Node, source: &str) -> Option<FunctionDef> {
    let is_library = def_node
        .children(&mut def_node.walk())
        .any(|c| c.kind() == "library_keyword");

    // Find the function definition node (string or numeric)
    let func_def = def_node.children(&mut def_node.walk()).find(|c| {
        c.kind() == "string_function_definition" || c.kind() == "numeric_function_definition"
    })?;

    // Find function_name within the definition
    let name_node = func_def
        .children(&mut func_def.walk())
        .find(|c| c.kind() == "function_name")?;

    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();
    let selection_range = node_range(name_node);
    let range = node_range(def_node);

    // Extract parameters
    let params = func_def
        .children(&mut func_def.walk())
        .find(|c| c.kind() == "parameter_list")
        .map(|pl| extract_params(pl, source))
        .unwrap_or_default();

    Some(FunctionDef {
        name,
        range,
        selection_range,
        is_library,
        params,
    })
}

fn extract_params(param_list: Node, source: &str) -> Vec<ParamInfo> {
    let mut params = Vec::new();
    let mut cursor = param_list.walk();

    for child in param_list.children(&mut cursor) {
        let is_optional = child.kind() == "optional_parameter";
        if child.kind() != "required_parameter" && child.kind() != "optional_parameter" {
            continue;
        }

        // required_parameter / optional_parameter wraps a parameter node
        let param_node = match child
            .children(&mut child.walk())
            .find(|c| c.kind() == "parameter")
        {
            Some(n) => n,
            None => continue,
        };

        if let Some(info) = extract_one_param(param_node, is_optional, source) {
            params.push(info);
        }
    }

    params
}

fn extract_one_param(param_node: Node, is_optional: bool, source: &str) -> Option<ParamInfo> {
    // Check for & (pass-by-reference) — it's an anonymous child
    let is_reference = param_node
        .children(&mut param_node.walk())
        .any(|c| !c.is_named() && c.utf8_text(source.as_bytes()).ok() == Some("&"));

    // Find the typed parameter child
    let mut cursor = param_node.walk();
    for child in param_node.named_children(&mut cursor) {
        match child.kind() {
            "numeric_parameter" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::Numeric,
                    is_optional,
                    is_reference,
                });
            }
            "string_parameter" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::String,
                    is_optional,
                    is_reference,
                });
            }
            "string_array_parameter" | "stringarray" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::StringArray,
                    is_optional,
                    is_reference,
                });
            }
            "number_array_parameter" | "numberarray" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::NumericArray,
                    is_optional,
                    is_reference,
                });
            }
            _ => {}
        }
    }
    None
}

fn find_identifier_name(node: Node, source: &str) -> Option<String> {
    // DFS to find a stringidentifier or numberidentifier leaf
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "stringidentifier" || n.kind() == "numberidentifier" {
            return n.utf8_text(source.as_bytes()).ok().map(|s| s.to_string());
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

    fn parse_and_extract(source: &str) -> Vec<FunctionDef> {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        extract_definitions(&tree, source)
    }

    #[test]
    fn simple_numeric_function() {
        let defs = parse_and_extract("def fnAdd(A, B) = A + B\n");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "fnAdd");
        assert!(!defs[0].is_library);
        assert_eq!(defs[0].params.len(), 2);
        assert_eq!(defs[0].params[0].name, "A");
        assert_eq!(defs[0].params[0].kind, ParamKind::Numeric);
        assert!(!defs[0].params[0].is_optional);
        assert!(!defs[0].params[0].is_reference);
    }

    #[test]
    fn library_string_function() {
        let defs = parse_and_extract("def library fnGetName$(Id)\nfnend\n");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "fnGetName$");
        assert!(defs[0].is_library);
        assert_eq!(defs[0].params.len(), 1);
    }

    #[test]
    fn params_with_types_and_modifiers() {
        let defs = parse_and_extract("def fnTest(A, &B$*20, mat C$, mat D; E)\nfnend\n");
        assert_eq!(defs.len(), 1);
        let params = &defs[0].params;
        assert_eq!(params.len(), 5);

        // A — numeric, required, not reference
        assert_eq!(params[0].name, "A");
        assert_eq!(params[0].kind, ParamKind::Numeric);
        assert!(!params[0].is_optional);
        assert!(!params[0].is_reference);

        // &B$ — string, required, reference
        assert_eq!(params[1].name, "B$");
        assert_eq!(params[1].kind, ParamKind::String);
        assert!(!params[1].is_optional);
        assert!(params[1].is_reference);

        // mat C$ — string array, required
        assert_eq!(params[2].name, "C$");
        assert_eq!(params[2].kind, ParamKind::StringArray);
        assert!(!params[2].is_optional);

        // mat D — numeric array, required
        assert_eq!(params[3].name, "D");
        assert_eq!(params[3].kind, ParamKind::NumericArray);
        assert!(!params[3].is_optional);

        // E — numeric, optional (after semicolon)
        assert_eq!(params[4].name, "E");
        assert_eq!(params[4].kind, ParamKind::Numeric);
        assert!(params[4].is_optional);
    }

    #[test]
    fn multiple_functions() {
        let source = "\
def fnFirst(X)
fnend
def library fnSecond$(A$)
fnend
";
        let defs = parse_and_extract(source);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "fnFirst");
        assert!(!defs[0].is_library);
        assert_eq!(defs[1].name, "fnSecond$");
        assert!(defs[1].is_library);
    }

    #[test]
    fn no_params() {
        let defs = parse_and_extract("def fnNoArgs = 42\n");
        assert_eq!(defs.len(), 1);
        assert!(defs[0].params.is_empty());
    }

    #[test]
    fn empty_source() {
        let defs = parse_and_extract("");
        assert!(defs.is_empty());
    }
}
