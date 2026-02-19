use std::collections::HashMap;

use tower_lsp::lsp_types::Range;
use tree_sitter::{Node, Tree};

use crate::parser::node_range;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionDef {
    pub name: String,
    pub range: Range,
    pub selection_range: Range,
    pub is_library: bool,
    pub is_import_only: bool,
    pub params: Vec<ParamInfo>,
    pub has_param_substitution: bool,
    pub documentation: Option<String>,
    pub return_documentation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamInfo {
    pub name: String,
    pub kind: ParamKind,
    pub is_optional: bool,
    pub is_reference: bool,
    pub documentation: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    Numeric,
    String,
    NumericArray,
    StringArray,
}

impl FunctionDef {
    /// Returns the visible params, truncated before the first `___` parameter.
    pub fn visible_params(&self) -> &[ParamInfo] {
        let end = self
            .params
            .iter()
            .position(|p| p.name.starts_with("___"))
            .unwrap_or(self.params.len());
        &self.params[..end]
    }

    pub fn format_signature(&self) -> String {
        let visible = self.visible_params();
        if visible.is_empty() {
            return self.name.clone();
        }
        let params: Vec<String> = visible.iter().map(|p| p.format_label()).collect();
        format!("{}({})", self.name, params.join(", "))
    }

    pub fn format_signature_with_offsets(&self) -> (String, Vec<[u32; 2]>) {
        let visible = self.visible_params();
        if visible.is_empty() {
            return (self.name.clone(), Vec::new());
        }

        let mut label = self.name.clone();
        label.push('(');
        let mut offsets = Vec::with_capacity(visible.len());

        for (i, param) in visible.iter().enumerate() {
            if i > 0 {
                label.push_str(", ");
            }
            let start = label.len() as u32;
            label.push_str(&param.format_label());
            let end = label.len() as u32;
            offsets.push([start, end]);
        }
        label.push(')');

        (label, offsets)
    }
}

impl ParamInfo {
    pub fn format_label(&self) -> String {
        let mut s = String::new();
        if self.is_optional {
            s.push('[');
        }
        if matches!(self.kind, ParamKind::NumericArray | ParamKind::StringArray) {
            s.push_str("mat ");
        }
        if self.is_reference {
            s.push('&');
        }
        s.push_str(&self.name);
        if self.is_optional {
            s.push(']');
        }
        s
    }
}

pub fn extract_definitions(tree: &Tree, source: &str) -> Vec<FunctionDef> {
    let mut defs = Vec::new();
    let root = tree.root_node();
    collect_def_statements(root, source, &mut defs);
    defs
}

fn collect_def_statements(node: Node, source: &str, defs: &mut Vec<FunctionDef>) {
    match node.kind() {
        "def_statement" => {
            if let Some(def) = extract_one_def(node, source) {
                defs.push(def);
            }
            return;
        }
        "library_statement" => {
            collect_library_imports(node, source, defs);
            return;
        }
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_def_statements(child, source, defs);
    }
}

/// Extract function names declared via `LIBRARY path: fnA, fnB$` import statements.
fn collect_library_imports(lib_node: Node, source: &str, defs: &mut Vec<FunctionDef>) {
    let mut cursor = lib_node.walk();
    for child in lib_node.children(&mut cursor) {
        if child.kind() == "library_function_list" {
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if grandchild.kind() == "function_name" {
                    if let Ok(name) = grandchild.utf8_text(source.as_bytes()) {
                        defs.push(FunctionDef {
                            name: name.to_string(),
                            range: node_range(lib_node),
                            selection_range: node_range(grandchild),
                            is_library: true,
                            is_import_only: true,
                            params: Vec::new(),
                            has_param_substitution: false,
                            documentation: None,
                            return_documentation: None,
                        });
                    }
                }
            }
        }
    }
}

/// Find the doc_comment text for a def_statement node by looking at the
/// immediately preceding sibling line.
fn find_doc_comment<'a>(def_node: Node<'a>, source: &'a str) -> Option<&'a str> {
    // def_statement is inside a line node
    let line_node = def_node.parent()?;
    if line_node.kind() != "line" {
        return None;
    }
    // Get the previous sibling line
    let prev_line = line_node.prev_sibling()?;
    if prev_line.kind() != "line" {
        return None;
    }
    // Look for a doc_comment child in that line
    let mut cursor = prev_line.walk();
    for child in prev_line.children(&mut cursor) {
        if child.kind() == "doc_comment" {
            return child.utf8_text(source.as_bytes()).ok();
        }
    }
    None
}

struct DocComment {
    description: Option<String>,
    return_doc: Option<String>,
    param_docs: Vec<(String, String)>, // (name, documentation)
}

fn parse_doc_comment(raw: &str) -> DocComment {
    // Strip /** and */
    let inner = raw.trim_start_matches("/**").trim_end_matches("*/").trim();

    let mut description_lines = Vec::new();
    let mut param_docs = Vec::new();
    let mut return_doc = None;
    let mut in_tags = false;

    for line in inner.lines() {
        // Strip leading whitespace and optional leading *
        let trimmed = line.trim().trim_start_matches('*').trim();

        if trimmed.starts_with("@param") {
            in_tags = true;
            let rest = trimmed.trim_start_matches("@param").trim();
            // Format: @param name description
            if let Some((name, doc)) = rest.split_once(char::is_whitespace) {
                param_docs.push((name.trim().to_string(), doc.trim().to_string()));
            } else if !rest.is_empty() {
                param_docs.push((rest.to_string(), String::new()));
            }
        } else if trimmed.starts_with("@return") {
            in_tags = true;
            let rest = trimmed
                .trim_start_matches("@returns")
                .trim_start_matches("@return")
                .trim();
            return_doc = Some(rest.to_string());
        } else if !in_tags && !trimmed.is_empty() {
            description_lines.push(trimmed.to_string());
        }
    }

    let description = if description_lines.is_empty() {
        None
    } else {
        Some(description_lines.join(" "))
    };

    DocComment {
        description,
        return_doc,
        param_docs,
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
    let param_list_node = func_def
        .children(&mut func_def.walk())
        .find(|c| c.kind() == "parameter_list");
    let mut params = param_list_node
        .map(|pl| extract_params(pl, source))
        .unwrap_or_default();
    let has_param_substitution = param_list_node.is_some_and(|pl| has_substitution(pl));

    // Parse doc comment if present
    let (documentation, return_documentation) =
        if let Some(raw) = find_doc_comment(def_node, source) {
            let doc = parse_doc_comment(raw);
            // Attach param docs to matching ParamInfo entries
            for (pname, pdoc) in &doc.param_docs {
                if let Some(param) = params
                    .iter_mut()
                    .find(|p| p.name.eq_ignore_ascii_case(pname))
                {
                    param.documentation = Some(pdoc.clone());
                }
            }
            (doc.description, doc.return_doc)
        } else {
            (None, None)
        };

    Some(FunctionDef {
        name,
        range,
        selection_range,
        is_library,
        is_import_only: false,
        params,
        has_param_substitution,
        documentation,
        return_documentation,
    })
}

/// Check if a parameter_list contains any param_substitution nodes (e.g. `[[Name]]`).
fn has_substitution(param_list: Node) -> bool {
    let mut cursor = param_list.walk();
    for child in param_list.children(&mut cursor) {
        if child.kind() == "required_parameter" || child.kind() == "optional_parameter" {
            let mut inner = child.walk();
            for grandchild in child.children(&mut inner) {
                if grandchild.kind() == "parameter" {
                    let mut param_cursor = grandchild.walk();
                    for param_child in grandchild.named_children(&mut param_cursor) {
                        if param_child.kind() == "param_substitution" {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
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
                    documentation: None,
                });
            }
            "string_parameter" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::String,
                    is_optional,
                    is_reference,
                    documentation: None,
                });
            }
            "string_array_parameter" | "stringarray" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::StringArray,
                    is_optional,
                    is_reference,
                    documentation: None,
                });
            }
            "number_array_parameter" | "numberarray" => {
                let name = find_identifier_name(child, source)?;
                return Some(ParamInfo {
                    name,
                    kind: ParamKind::NumericArray,
                    is_optional,
                    is_reference,
                    documentation: None,
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

/// Walk all `library_statement` nodes and return a mapping of
/// lowercase function name → normalized library path.
pub fn extract_library_links(tree: &Tree, source: &str) -> HashMap<String, String> {
    let mut links = HashMap::new();
    collect_library_links(tree.root_node(), source, &mut links);
    links
}

fn collect_library_links(node: Node, source: &str, links: &mut HashMap<String, String>) {
    if node.kind() == "library_statement" {
        if let Some(path_node) = node.child_by_field_name("path") {
            if let Some(raw) = extract_string_literal(path_node, source) {
                let normalized = normalize_library_path(&raw);
                // Collect function names from library_function_list
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "library_function_list" {
                        let mut inner = child.walk();
                        for grandchild in child.children(&mut inner) {
                            if grandchild.kind() == "function_name" {
                                if let Ok(name) = grandchild.utf8_text(source.as_bytes()) {
                                    links.insert(
                                        name.to_ascii_lowercase(),
                                        normalized.clone(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_library_links(child, source, links);
    }
}

/// DFS for a `"string"` leaf node and return its text with quotes stripped.
fn extract_string_literal(node: Node, source: &str) -> Option<String> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if n.kind() == "string" {
            let text = n.utf8_text(source.as_bytes()).ok()?;
            // Strip surrounding quotes (either " or ')
            let trimmed = text
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .or_else(|| {
                    text.strip_prefix('\'')
                        .and_then(|s| s.strip_suffix('\''))
                });
            return trimmed.map(|s| s.to_string());
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

/// Normalize a library path: backslash → `/`, lowercase, strip extension.
pub fn normalize_library_path(raw: &str) -> String {
    let s = raw.replace('\\', "/").to_ascii_lowercase();
    s.strip_suffix(".brs")
        .or_else(|| s.strip_suffix(".wbs"))
        .or_else(|| s.strip_suffix(".dll"))
        .unwrap_or(&s)
        .to_string()
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

    #[test]
    fn doc_comment_parsed() {
        let source = "\
/** Adds two numbers
  * @param A First number
  * @param B Second number
  * @returns The sum
  */
def fnAdd(A, B) = A + B
";
        let defs = parse_and_extract(source);
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].documentation.as_deref(), Some("Adds two numbers"));
        assert_eq!(defs[0].return_documentation.as_deref(), Some("The sum"));
        assert_eq!(
            defs[0].params[0].documentation.as_deref(),
            Some("First number")
        );
        assert_eq!(
            defs[0].params[1].documentation.as_deref(),
            Some("Second number")
        );
    }

    #[test]
    fn no_doc_comment() {
        let defs = parse_and_extract("def fnPlain(X) = X\n");
        assert_eq!(defs.len(), 1);
        assert!(defs[0].documentation.is_none());
        assert!(defs[0].return_documentation.is_none());
    }

    #[test]
    fn library_import_statement() {
        let defs = parse_and_extract("library \"vol002\\rtflib.dll\": fnRTF, fnRTFStart$\n");
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].name, "fnRTF");
        assert!(defs[0].is_library);
        assert!(defs[0].params.is_empty());
        assert_eq!(defs[1].name, "fnRTFStart$");
        assert!(defs[1].is_library);
        assert!(defs[1].params.is_empty());
    }

    #[test]
    fn format_signature_simple() {
        let defs = parse_and_extract("def fnCalc(A, B) = A + B\n");
        assert_eq!(defs[0].format_signature(), "fnCalc(A, B)");
    }

    #[test]
    fn format_signature_modifiers() {
        let defs = parse_and_extract("def fnTest(&A$, mat B; C)\nfnend\n");
        assert_eq!(defs[0].format_signature(), "fnTest(&A$, mat B, [C])");
    }

    #[test]
    fn format_signature_no_params() {
        let defs = parse_and_extract("def fnConst = 42\n");
        assert_eq!(defs[0].format_signature(), "fnConst");
    }

    #[test]
    fn format_signature_offsets() {
        let defs = parse_and_extract("def fnCalc(A, B) = A + B\n");
        let (label, offsets) = defs[0].format_signature_with_offsets();
        assert_eq!(label, "fnCalc(A, B)");
        assert_eq!(offsets.len(), 2);
        assert_eq!(&label[offsets[0][0] as usize..offsets[0][1] as usize], "A");
        assert_eq!(&label[offsets[1][0] as usize..offsets[1][1] as usize], "B");
    }

    #[test]
    fn semicolon_ampersand_params() {
        // Test the `;& pattern (semicolon immediately followed by ampersand)
        let defs =
            parse_and_extract("def fnPause(Howlong;&thekey$,&function,___,looping)\nfnend\n");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "fnPause");

        let params = &defs[0].params;
        assert_eq!(params.len(), 5);

        assert_eq!(params[0].name, "Howlong");
        assert_eq!(params[0].kind, ParamKind::Numeric);
        assert!(!params[0].is_optional);
        assert!(!params[0].is_reference);

        assert_eq!(params[1].name, "thekey$");
        assert_eq!(params[1].kind, ParamKind::String);
        assert!(params[1].is_optional);
        assert!(params[1].is_reference);

        assert_eq!(params[2].name, "function");
        assert_eq!(params[2].kind, ParamKind::Numeric);
        assert!(params[2].is_optional);
        assert!(params[2].is_reference);

        // visible_params truncates at ___
        let visible = defs[0].visible_params();
        assert_eq!(visible.len(), 3);

        assert_eq!(
            defs[0].format_signature(),
            "fnPause(Howlong, [&thekey$], [&function])"
        );
    }

    fn parse_and_extract_links(source: &str) -> HashMap<String, String> {
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        extract_library_links(&tree, source)
    }

    #[test]
    fn library_links_basic() {
        let links = parse_and_extract_links(
            "library \"vol002\\rtflib\": fnRTF, fnRTFStart$\n",
        );
        assert_eq!(links.get("fnrtf").unwrap(), "vol002/rtflib");
        assert_eq!(links.get("fnrtfstart$").unwrap(), "vol002/rtflib");
    }

    #[test]
    fn library_links_with_extension() {
        let links = parse_and_extract_links("library \"custlib.brs\": fnCalc\n");
        assert_eq!(links.get("fncalc").unwrap(), "custlib");
    }

    #[test]
    fn library_links_multiple_statements() {
        let source = "\
library \"vol002\\rtflib\": fnRTF
library \"custlib\": fnCalc
";
        let links = parse_and_extract_links(source);
        assert_eq!(links.len(), 2);
        assert_eq!(links.get("fnrtf").unwrap(), "vol002/rtflib");
        assert_eq!(links.get("fncalc").unwrap(), "custlib");
    }

    #[test]
    fn library_links_no_path() {
        let links = parse_and_extract_links("library: fnFoo\n");
        assert!(links.is_empty());
    }

    #[test]
    fn normalize_library_path_cases() {
        assert_eq!(normalize_library_path("VOL002\\RTFLib"), "vol002/rtflib");
        assert_eq!(normalize_library_path("custlib.brs"), "custlib");
        assert_eq!(normalize_library_path("some/path.DLL"), "some/path");
        assert_eq!(normalize_library_path("simple"), "simple");
    }
}
