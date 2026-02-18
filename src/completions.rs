use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use tower_lsp::lsp_types::*;

use crate::backend::DocumentState;
use crate::builtins;
use crate::extract;
use crate::parser;
use crate::workspace::WorkspaceIndex;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CompletionData {
    #[serde(rename = "builtin")]
    Builtin { name: String, overload: usize },
    #[serde(rename = "local")]
    Local { name: String, uri: String },
    #[serde(rename = "workspace")]
    Workspace { name: String },
}

pub fn format_builtin_docs(b: &builtins::BuiltinFunction) -> String {
    let sig = b.format_signature();
    let mut md_parts = vec![format!("```br\n{sig}\n```")];
    if let Some(doc) = &b.documentation {
        md_parts.push(doc.clone());
    }
    let param_docs: Vec<String> = b
        .params
        .iter()
        .filter_map(|p| {
            p.documentation
                .as_ref()
                .map(|d| format!("*@param* `{}` \u{2014} {d}", p.name))
        })
        .collect();
    if !param_docs.is_empty() {
        md_parts.push(param_docs.join("\n\n"));
    }
    md_parts.join("\n\n")
}

pub fn format_function_docs(d: &extract::FunctionDef) -> String {
    let sig = d.format_signature();
    let mut md_parts = vec![format!("```br\n{sig}\n```")];
    if let Some(doc) = &d.documentation {
        md_parts.push(doc.clone());
    }
    let param_docs: Vec<String> = d
        .params
        .iter()
        .filter_map(|p| {
            p.documentation
                .as_ref()
                .map(|doc| format!("*@param* `{}` \u{2014} {doc}", p.format_label()))
        })
        .collect();
    if !param_docs.is_empty() {
        md_parts.push(param_docs.join("\n\n"));
    }
    if let Some(ret) = &d.return_documentation {
        md_parts.push(format!("*@returns* \u{2014} {ret}"));
    }
    md_parts.join("\n\n")
}

pub fn get_completions(
    doc: &DocumentState,
    uri: &str,
    position: Position,
    workspace_index: &WorkspaceIndex,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    items.extend(statement_completions());
    items.extend(keyword_completions());
    items.extend(builtin_function_completions());

    if let Some(tree) = doc.tree.as_ref() {
        items.extend(local_variable_completions(tree, &doc.source, position));
        items.extend(local_function_completions(tree, &doc.source, uri));
    }

    items.extend(library_function_completions(uri, workspace_index));
    items
}

// ---------------------------------------------------------------------------
// Statements (#9)
// ---------------------------------------------------------------------------

struct StatementEntry {
    name: &'static str,
    description: &'static str,
    documentation: &'static str,
    doc_url: &'static str,
    example: &'static str,
}

const STATEMENTS: &[StatementEntry] = &[
    StatementEntry {
        name: "do",
        description: "",
        documentation: "",
        doc_url: "",
        example: "",
    },
    StatementEntry {
        name: "loop",
        description: "",
        documentation: "",
        doc_url: "",
        example: "",
    },
    StatementEntry {
        name: "if",
        description: "",
        documentation: "",
        doc_url: "",
        example: "",
    },
    StatementEntry {
        name: "end if",
        description: "",
        documentation: "",
        doc_url: "",
        example: "",
    },
    StatementEntry {
        name: "def",
        description: "Def Statement",
        documentation: "Defines function.",
        doc_url: "http://www.brwiki.com/index.php?title=Def",
        example: "def fnfoo(bar)\n\t! body\nfnend",
    },
    StatementEntry {
        name: "def library",
        description: "Def Library Fn ... fnend",
        documentation: "Define library function",
        doc_url: "http://www.brwiki.com/index.php?title=Def",
        example: "",
    },
    StatementEntry {
        name: "Chain",
        description: "Chain {<program name>|\"PROC=<name>\"|\"SUPROC=<name>\"} ...",
        documentation: "Loads and Runs the target program, immediately ending the current program. Optionally passes variables and files into the called program.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Chain",
        example: "",
    },
    StatementEntry {
        name: "Close",
        description: "Close {#<file/window number>} [,Free|Drop] [, ...] :",
        documentation: "The Close (CL) statement deactivates access to a data or window file for input or output.",
        doc_url: "http://www.brwiki.com/index.php?search=Close",
        example: "",
    },
    StatementEntry {
        name: "Continue",
        description: "Continue Statement",
        documentation: "Jumps to the line following the line that had the most recent error. Used to continue in an Error Handler.",
        doc_url: "http://www.brwiki.com/index.php?search=Continue",
        example: "",
    },
    StatementEntry {
        name: "Data",
        description: "Data {\"<string constant>\"|<numeric constant>}[,...]",
        documentation: "The Data statement can be used to populate the values of variables.",
        doc_url: "http://www.brwiki.com/index.php?search=Data",
        example: "",
    },
    StatementEntry {
        name: "Delete",
        description: "Delete",
        documentation: "Deletes the currently locked record from the identified data file..",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Delete_(statement)",
        example: "",
    },
    StatementEntry {
        name: "Dim",
        description: "Dim",
        documentation: "Declares Variables and Arrays. Arrays must be declared if they have other then 10 messages.",
        doc_url: "http://www.brwiki.com/index.php?search=Dim",
        example: "",
    },
    StatementEntry {
        name: "Display",
        description: "Display [Menu|Buttons] ...",
        documentation: "Display or Update the Windows Menu, or the Button Rows.",
        doc_url: "http://www.brwiki.com/index.php?search=Display",
        example: "",
    },
    StatementEntry {
        name: "End",
        description: "End",
        documentation: "Ends your program (continuing with any proc files that ran your program, or stopping if your program wasn't run from a proc.)",
        doc_url: "http://www.brwiki.com/index.php?search=End",
        example: "",
    },
    StatementEntry {
        name: "Execute",
        description: "Execute \"BR Command\"",
        documentation: "Executes a Command from within one of your programs.",
        doc_url: "http://www.brwiki.com/index.php?search=Execute",
        example: "",
    },
    StatementEntry {
        name: "Exit",
        description: "Exit <error condition line ref>[,...]",
        documentation: "Works in conjunction with the Exit error condition to list a bunch of error handlers in one place.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Exit",
        example: "",
    },
    StatementEntry {
        name: "Exit Do",
        description: "Exit Do Statement",
        documentation: "Jumps out of a do loop to the line following the loop.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Exit_do",
        example: "",
    },
    StatementEntry {
        name: "Fnend",
        description: "Fnend Statement",
        documentation: "The FnEnd (FN) and End Def statements indicates the end of a definition of a multi-lined user defined function.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Fnend",
        example: "",
    },
    StatementEntry {
        name: "Print",
        description: "Print Statement",
        documentation: "Prints a line to the console, or to a specific file.",
        doc_url: "http://www.brwiki.com/index.php?search=Print",
        example: "",
    },
    StatementEntry {
        name: "Input",
        description: "Input <Variables>",
        documentation: "Reads text from the user or from a display file (like a text file). It can also read text from a proc file, if the program is called from a proc.",
        doc_url: "http://www.brwiki.com/index.php?search=Input",
        example: "",
    },
    StatementEntry {
        name: "Linput",
        description: "Linput <StringVariable>",
        documentation: "Reads a line of text from a display file. This is useful for parsing CSV files and other files generated by external applications.",
        doc_url: "http://www.brwiki.com/index.php?search=Linput",
        example: "",
    },
    StatementEntry {
        name: "Input",
        description: "Input Fields",
        documentation: "Activates a bunch of controls on the screen and pauses execution, allowing the user to interact with them. This is the primary way that BR programs interact with the User.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Input_Fields",
        example: "",
    },
    StatementEntry {
        name: "Rinput",
        description: "Rinput Fields",
        documentation: "Updates and then activates a bunch of controls on the screen and pauses execution, allowing the user to interact with them. This is the primary way that BR programs interact with the User.",
        doc_url: "http://www.brwiki.com/index.php?search=Rinput",
        example: "",
    },
    StatementEntry {
        name: "Input",
        description: "Input Select",
        documentation: "Activates a bunch of controls and allows the user to select one of them.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Input_Select",
        example: "",
    },
    StatementEntry {
        name: "Rinput",
        description: "Rinput Select",
        documentation: "Activates and Displays a bunch of controls and allows the user to select one of them.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Rinput_select",
        example: "",
    },
    StatementEntry {
        name: "For",
        description: "Form",
        documentation: "The Form statement is used in conjunction with PRINT, WRITE, REWRITE, READ or REREAD statements to format input or output. FORM controls the size, location, field length and format of input or output.",
        doc_url: "http://www.brwiki.com/index.php?search=Form",
        example: "",
    },
    StatementEntry {
        name: "Gosub",
        description: "Gosub <LineLabel/LineNumber>",
        documentation: "Calls a subroutine, which runs until it encounters a return statement, at which point it returns here.",
        doc_url: "http://www.brwiki.com/index.php?search=Gosub",
        example: "",
    },
    StatementEntry {
        name: "Goto",
        description: "Goto <LineLabel/LineNumber>",
        documentation: "Jumps to the target line and continues running from there. (Try not to use Goto Statements. This is not the 80s.).",
        doc_url: "http://www.brwiki.com/index.php?search=Goto",
        example: "",
    },
    StatementEntry {
        name: "Library",
        description: "Library \"<Library>\" : <fnFunction1> [, fnFunction2] [, ...]",
        documentation: "Loads a BR Libary, allowing access to the library functions in it.",
        doc_url: "http://www.brwiki.com/index.php?search=Library",
        example: "",
    },
    StatementEntry {
        name: "Mat",
        description: "Mat <array name> [(<dimension>[,...])] = ....",
        documentation: "The Mat statement is used for working with Arrays. Its used to resize arrays, sort them (in conjunction with AIDX or DIDX), copy them, and process them in lots of other ways.",
        doc_url: "http://www.brwiki.com/index.php?search=Mat",
        example: "",
    },
    StatementEntry {
        name: "On",
        description: "On Statement",
        documentation: "",
        doc_url: "",
        example: "",
    },
    StatementEntry {
        name: "Open",
        description: "Open #<FileNumber> \"Name=...\"",
        documentation: "Opens a file or window or http connection or comm port.",
        doc_url: "http://www.brwiki.com/index.php?search=Open",
        example: "",
    },
    StatementEntry {
        name: "Pause",
        description: "Pause",
        documentation: "Pauses program execution allows the programmer to interact with the program in the Command Console.",
        doc_url: "http://brwiki2.brulescorp.com/index.php?title=Pause",
        example: "",
    },
    StatementEntry {
        name: "Randomize",
        description: "Randomize",
        documentation: "Generates a new Random Number Seed for the Random Number Generator (based on the system clock so as to be truly random).",
        doc_url: "http://www.brwiki.com/index.php?search=Randomize",
        example: "",
    },
    StatementEntry {
        name: "Read",
        description: "Read Statement",
        documentation: "Reads data",
        doc_url: "http://www.brwiki.com/index.php?search=Read",
        example: "",
    },
    StatementEntry {
        name: "Reread",
        description: "Reread  #<file number> [, USING {<formStatement>}] : <Variables>",
        documentation: "Rereads the previous record read again, in the selected data file or data statements, storing the information in the variables provided.",
        doc_url: "http://www.brwiki.com/index.php?search=Reread",
        example: "",
    },
    StatementEntry {
        name: "Write",
        description: "Write  #<file number> [, USING {<formStatement>}] : <Variables>",
        documentation: "Adds a record to the file containing the information from the variables you list.",
        doc_url: "http://www.brwiki.com/index.php?search=Write",
        example: "",
    },
    StatementEntry {
        name: "Rewrite",
        description: "Rewrite  #<file number> [, USING {<formStatement>}] : <Variables>",
        documentation: "Updates the record that is locked in the file (usually the last record read), with the data in the variables now.",
        doc_url: "http://www.brwiki.com/index.php?search=Rewrite",
        example: "",
    },
    StatementEntry {
        name: "Restore",
        description: "Restore  #<file number> [,<Key|Rec|Pos|Search> = <SearchValue|Position>:",
        documentation: "Jumps to the beginning (or other specified point) in the targeted file.",
        doc_url: "http://www.brwiki.com/index.php?search=Restore",
        example: "",
    },
    StatementEntry {
        name: "Retry",
        description: "Retry",
        documentation: "Jumps to the line that had the most recent error. Used to try again in an Error Handler.",
        doc_url: "http://www.brwiki.com/index.php?search=Retry",
        example: "",
    },
    StatementEntry {
        name: "Return",
        description: "Return",
        documentation: "Exits a Subroutine and returns control back up to the code following the Gosub statement.",
        doc_url: "http://www.brwiki.com/index.php?search=Return",
        example: "",
    },
    StatementEntry {
        name: "Scr_Freeze",
        description: "Scr_Freeze",
        documentation: "Stops the screen from updating, significantly increasing the speed of the programs. The screen starts running again at the next Input Statement or Scr_Thaw statement.",
        doc_url: "http://www.brwiki.com/index.php?search=Scr_freeze",
        example: "",
    },
    StatementEntry {
        name: "Scr_Thaw",
        description: "Scr_Thaw",
        documentation: "Causes the screen to refresh and begin updating again after it was frozen with a Scr_Freeze command.",
        doc_url: "http://www.brwiki.com/index.php?search=Scr_thaw",
        example: "",
    },
    StatementEntry {
        name: "Stop",
        description: "Stop",
        documentation: "Ends your program (continuing with any proc files that ran your program, or stopping if your program wasn't run from a proc.)",
        doc_url: "http://www.brwiki.com/index.php?search=Stop",
        example: "",
    },
    StatementEntry {
        name: "Trace",
        description: "Trace [On|Off|Print]",
        documentation: "Displays or outputs the line numbers as they're executed. Used for debugging code, but the modern debugging tools are much better.",
        doc_url: "http://www.brwiki.com/index.php?search=Trace",
        example: "",
    },
];

fn statement_completions() -> Vec<CompletionItem> {
    STATEMENTS
        .iter()
        .map(|s| {
            let mut md_parts = Vec::new();
            if !s.documentation.is_empty() {
                md_parts.push(s.documentation.to_string());
            }
            if !s.doc_url.is_empty() {
                md_parts.push(format!("[Documentation]({})", s.doc_url));
            }
            if !s.example.is_empty() {
                md_parts.push(format!("```br\n{}\n```", s.example));
            }
            let documentation = if md_parts.is_empty() {
                None
            } else {
                Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: md_parts.join("\n\n"),
                }))
            };

            CompletionItem {
                label: s.name.to_string(),
                kind: Some(CompletionItemKind::KEYWORD),
                detail: if s.description.is_empty() {
                    None
                } else {
                    Some(s.description.to_string())
                },
                documentation,
                ..Default::default()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Keywords (#10)
// ---------------------------------------------------------------------------

struct KeywordEntry {
    name: &'static str,
    documentation: &'static str,
}

const KEYWORDS: &[KeywordEntry] = &[
    KeywordEntry {
        name: "while",
        documentation: "",
    },
    KeywordEntry {
        name: "fields",
        documentation: "",
    },
    KeywordEntry {
        name: "until",
        documentation: "",
    },
    KeywordEntry {
        name: "wait",
        documentation: "The `WAIT=` parameter and TIMEOUT error trap can be used with `INPUT`/`RINPUT`/`LInput` statements to force releasing of records. This feature is useful for multi-user situations.",
    },
];

fn keyword_completions() -> Vec<CompletionItem> {
    KEYWORDS
        .iter()
        .map(|k| CompletionItem {
            label: k.name.to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            documentation: if k.documentation.is_empty() {
                None
            } else {
                Some(Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: k.documentation.to_string(),
                }))
            },
            ..Default::default()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Built-in functions (#11)
// ---------------------------------------------------------------------------

fn builtin_function_completions() -> Vec<CompletionItem> {
    let mut overload_counts: HashMap<String, usize> = HashMap::new();

    builtins::all()
        .map(|b| {
            let sig = b.format_signature();
            let detail = format!("(built-in) {sig}");

            let key = b.name.to_ascii_lowercase();
            let overload = *overload_counts.get(&key).unwrap_or(&0);
            *overload_counts.entry(key).or_insert(0) += 1;

            let data = serde_json::to_value(CompletionData::Builtin {
                name: b.name.clone(),
                overload,
            })
            .ok();

            CompletionItem {
                label: b.name.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(detail),
                documentation: None,
                data,
                ..Default::default()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Local variables (#12)
// ---------------------------------------------------------------------------

fn local_variable_completions(
    tree: &tree_sitter::Tree,
    source: &str,
    position: Position,
) -> Vec<CompletionItem> {
    let root = tree.root_node();

    let queries: &[(&str, &str)] = &[
        ("(stringarray name: (_) @name)", "string array"),
        ("(numberarray name: (_) @name)", "number array"),
        ("(stringreference name: (_) @name)", "string"),
        ("(numberreference name: (_) @name)", "number"),
    ];

    let mut seen = HashSet::new();
    let mut items = Vec::new();

    for &(query_str, type_label) in queries {
        let results = parser::run_query(query_str, root, source);
        for r in results {
            // Exclude the token at cursor position
            if r.range.start.line == position.line
                && r.range.start.character <= position.character
                && r.range.end.character >= position.character
                && r.range.end.line == position.line
            {
                continue;
            }

            let key = (r.text.to_ascii_lowercase(), type_label);
            if !seen.insert(key) {
                continue;
            }

            items.push(CompletionItem {
                label: r.text,
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(type_label.to_string()),
                ..Default::default()
            });
        }
    }

    items
}

// ---------------------------------------------------------------------------
// Local functions (#13)
// ---------------------------------------------------------------------------

fn local_function_completions(
    tree: &tree_sitter::Tree,
    source: &str,
    uri: &str,
) -> Vec<CompletionItem> {
    let defs = extract::extract_definitions(tree, source);
    defs.into_iter()
        .filter(|d| !d.is_import_only)
        .map(|d| {
            let sig = d.format_signature();
            let detail = format!("(local) {sig}");

            let data = serde_json::to_value(CompletionData::Local {
                name: d.name.clone(),
                uri: uri.to_string(),
            })
            .ok();

            CompletionItem {
                label: d.name,
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(detail),
                documentation: None,
                data,
                ..Default::default()
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Library (workspace) functions (#14)
// ---------------------------------------------------------------------------

fn library_function_completions(
    current_uri: &str,
    index: &WorkspaceIndex,
) -> Vec<CompletionItem> {
    index
        .unique_functions(current_uri)
        .into_iter()
        .map(|s| {
            let sig = s.def.format_signature();
            let detail = format!("(library) {sig}");

            // Extract filename from URI for label_details
            let filename = s
                .uri
                .path_segments()
                .and_then(|mut segs| segs.next_back().map(|s| s.to_string()))
                .unwrap_or_default();

            let data = serde_json::to_value(CompletionData::Workspace {
                name: s.def.name.clone(),
            })
            .ok();

            CompletionItem {
                label: s.def.name.clone(),
                kind: Some(CompletionItemKind::FUNCTION),
                detail: Some(detail),
                label_details: Some(CompletionItemLabelDetails {
                    description: Some(filename),
                    detail: None,
                }),
                documentation: None,
                data,
                ..Default::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser;
    use crate::workspace::WorkspaceIndex;
    use tower_lsp::lsp_types::Url;

    #[test]
    fn statement_completions_not_empty() {
        let items = statement_completions();
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i.kind == Some(CompletionItemKind::KEYWORD)));
    }

    #[test]
    fn statement_completions_includes_known_entries() {
        let items = statement_completions();
        let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(names.contains(&"def"));
        assert!(names.contains(&"Print"));
        assert!(names.contains(&"Gosub"));
        assert!(names.contains(&"end if"));
    }

    #[test]
    fn statement_completions_count() {
        let items = statement_completions();
        assert_eq!(items.len(), STATEMENTS.len());
    }

    #[test]
    fn keyword_completions_count() {
        let items = keyword_completions();
        assert_eq!(items.len(), 4);
        assert!(items.iter().all(|i| i.kind == Some(CompletionItemKind::KEYWORD)));
    }

    #[test]
    fn keyword_wait_has_docs() {
        let items = keyword_completions();
        let wait = items.iter().find(|i| i.label == "wait").unwrap();
        assert!(wait.documentation.is_some());
    }

    #[test]
    fn builtin_completions_count() {
        let items = builtin_function_completions();
        assert_eq!(items.len(), 115);
        assert!(items.iter().all(|i| i.kind == Some(CompletionItemKind::FUNCTION)));
    }

    #[test]
    fn builtin_completions_detail() {
        let items = builtin_function_completions();
        let val = items.iter().find(|i| i.label == "Val").unwrap();
        assert!(val.detail.as_ref().unwrap().starts_with("(built-in)"));
    }

    #[test]
    fn local_variable_basics() {
        let source = "let X$ = \"hello\"\nlet Y = 42\nlet Z$ = X$\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        let pos = Position {
            line: 99,
            character: 0,
        };
        let items = local_variable_completions(&tree, source, pos);
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i.kind == Some(CompletionItemKind::VARIABLE)));
    }

    #[test]
    fn local_variable_dedup() {
        let source = "let X$ = \"a\"\nlet Y$ = X$\nlet Z$ = X$\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        let pos = Position {
            line: 99,
            character: 0,
        };
        let items = local_variable_completions(&tree, source, pos);
        let x_count = items.iter().filter(|i| i.label.eq_ignore_ascii_case("X$")).count();
        assert_eq!(x_count, 1, "X$ should appear exactly once");
    }

    #[test]
    fn local_function_extraction() {
        let source = "def fnAdd(A, B) = A + B\ndef library fnCalc$(X$)\nfnend\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        let items = local_function_completions(&tree, source, "file:///test.brs");
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|i| i.kind == Some(CompletionItemKind::FUNCTION)));
        assert!(items.iter().any(|i| i.label == "fnAdd"));
        assert!(items.iter().any(|i| i.label == "fnCalc$"));
    }

    #[test]
    fn local_function_detail_format() {
        let source = "def fnAdd(A, B) = A + B\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        let items = local_function_completions(&tree, source, "file:///test.brs");
        let item = &items[0];
        assert_eq!(item.detail.as_deref(), Some("(local) fnAdd(A, B)"));
    }

    #[test]
    fn library_excludes_current_file() {
        let mut index = WorkspaceIndex::new();
        let uri_a = Url::parse("file:///workspace/a.brs").unwrap();
        let uri_b = Url::parse("file:///workspace/b.brs").unwrap();
        index.add_file(
            &uri_a,
            vec![make_test_def("fnFoo", false, false)],
        );
        index.add_file(
            &uri_b,
            vec![make_test_def("fnBar", false, false)],
        );

        let items = library_function_completions(uri_a.as_str(), &index);
        let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(!names.contains(&"fnFoo"), "should exclude current file");
        assert!(names.contains(&"fnBar"));
    }

    #[test]
    fn library_excludes_import_only() {
        let mut index = WorkspaceIndex::new();
        let uri_a = Url::parse("file:///workspace/a.brs").unwrap();
        let uri_b = Url::parse("file:///workspace/b.brs").unwrap();
        index.add_file(
            &uri_a,
            vec![make_test_def("fnReal", false, false)],
        );
        index.add_file(
            &uri_b,
            vec![
                make_test_def("fnLib", false, false),
                make_test_def("fnImport", false, true),
            ],
        );

        let items = library_function_completions(uri_a.as_str(), &index);
        let names: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
        assert!(names.contains(&"fnLib"));
        assert!(
            !names.contains(&"fnImport"),
            "should exclude import-only entries"
        );
    }

    #[test]
    fn library_label_details_has_filename() {
        let mut index = WorkspaceIndex::new();
        let uri = Url::parse("file:///workspace/utils.brs").unwrap();
        let current = "file:///workspace/main.brs";
        index.add_file(&uri, vec![make_test_def("fnUtil", false, false)]);

        let items = library_function_completions(current, &index);
        assert_eq!(items.len(), 1);
        let ld = items[0].label_details.as_ref().unwrap();
        assert_eq!(ld.description.as_deref(), Some("utils.brs"));
    }

    #[test]
    fn get_completions_smoke() {
        let source = "let X$ = \"hello\"\ndef fnFoo(A) = A\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None);
        let doc = DocumentState {
            rope: ropey::Rope::from_str(source),
            source: source.to_string(),
            tree,
        };
        let index = WorkspaceIndex::new();
        let pos = Position {
            line: 99,
            character: 0,
        };
        let items = get_completions(&doc, "file:///test.brs", pos, &index);
        // Should have statements + keywords + builtins + local vars + local fns
        assert!(items.len() > 100);
    }

    #[test]
    fn builtin_completions_no_docs() {
        let items = builtin_function_completions();
        assert!(
            items.iter().all(|i| i.documentation.is_none()),
            "builtin completions should defer docs to resolve"
        );
    }

    #[test]
    fn builtin_completions_have_data() {
        let items = builtin_function_completions();
        let val = items.iter().find(|i| i.label == "Val").unwrap();
        let data: CompletionData =
            serde_json::from_value(val.data.clone().unwrap()).unwrap();
        assert!(matches!(data, CompletionData::Builtin { ref name, .. } if name == "Val"));
    }

    #[test]
    fn local_function_no_docs() {
        let source = "def fnAdd(A, B) = A + B\n";
        let mut p = parser::new_parser();
        let tree = parser::parse(&mut p, source, None).unwrap();
        let items = local_function_completions(&tree, source, "file:///test.brs");
        assert!(
            items.iter().all(|i| i.documentation.is_none()),
            "local function completions should defer docs to resolve"
        );
    }

    #[test]
    fn library_dedup_by_name() {
        let mut index = WorkspaceIndex::new();
        let uri_a = Url::parse("file:///workspace/a.brs").unwrap();
        let uri_b = Url::parse("file:///workspace/b.brs").unwrap();
        let current = "file:///workspace/main.brs";
        index.add_file(&uri_a, vec![make_test_def("fnFoo", false, false)]);
        index.add_file(&uri_b, vec![make_test_def("fnFoo", false, false)]);

        let items = library_function_completions(current, &index);
        let foo_count = items.iter().filter(|i| i.label == "fnFoo").count();
        assert_eq!(foo_count, 1, "duplicate function names should be deduped");
    }

    #[test]
    fn library_dedup_prefers_library() {
        let mut index = WorkspaceIndex::new();
        let uri_a = Url::parse("file:///workspace/a.brs").unwrap();
        let uri_b = Url::parse("file:///workspace/b.brs").unwrap();
        let current = "file:///workspace/main.brs";
        index.add_file(&uri_a, vec![make_test_def("fnFoo", false, false)]);
        index.add_file(&uri_b, vec![make_test_def("fnFoo", true, false)]);

        let items = library_function_completions(current, &index);
        assert_eq!(items.len(), 1);
        let ld = items[0].label_details.as_ref().unwrap();
        assert_eq!(
            ld.description.as_deref(),
            Some("b.brs"),
            "should pick the is_library entry from b.brs"
        );
    }

    #[test]
    fn library_completions_no_docs() {
        let mut index = WorkspaceIndex::new();
        let uri = Url::parse("file:///workspace/utils.brs").unwrap();
        let current = "file:///workspace/main.brs";
        index.add_file(&uri, vec![make_test_def("fnUtil", false, false)]);

        let items = library_function_completions(current, &index);
        assert!(
            items.iter().all(|i| i.documentation.is_none()),
            "library completions should defer docs to resolve"
        );
    }

    fn make_test_def(
        name: &str,
        is_library: bool,
        is_import_only: bool,
    ) -> extract::FunctionDef {
        extract::FunctionDef {
            name: name.to_string(),
            range: Range::default(),
            selection_range: Range::default(),
            is_library,
            is_import_only,
            params: vec![],
            has_param_substitution: false,
            documentation: None,
            return_documentation: None,
        }
    }
}
