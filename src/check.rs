use std::path::{Path, PathBuf};

use rayon::prelude::*;
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, Range, Url};
use walkdir::WalkDir;

use crate::{diagnostics, parser, workspace};

/// A diagnostic decoupled from LSP types, usable from both CLI and server paths.
#[derive(Debug)]
pub struct FileDiagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub severity: String,
    pub message: String,
}

fn severity_str(severity: Option<DiagnosticSeverity>) -> &'static str {
    match severity {
        Some(DiagnosticSeverity::ERROR) => "error",
        Some(DiagnosticSeverity::WARNING) => "warning",
        Some(DiagnosticSeverity::INFORMATION) => "information",
        Some(DiagnosticSeverity::HINT) => "hint",
        _ => "unknown",
    }
}

fn range_to_1based(range: &Range) -> (u32, u32, u32, u32) {
    (
        range.start.line + 1,
        range.start.character + 1,
        range.end.line + 1,
        range.end.character + 1,
    )
}

/// Check a single file for diagnostics.
pub fn check_file(path: &Path) -> Vec<FileDiagnostic> {
    let source = match workspace::read_br_file(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };

    let mut ts_parser = parser::new_parser();
    let tree = match parser::parse(&mut ts_parser, &source, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut lsp_diags = parser::collect_diagnostics(&tree, &source);
    lsp_diags.extend(diagnostics::collect_function_diagnostics(&tree, &source));

    let file_str = path.display().to_string();

    lsp_diags
        .into_iter()
        .map(|d| {
            let (line, column, end_line, end_column) = range_to_1based(&d.range);
            FileDiagnostic {
                file: file_str.clone(),
                line,
                column,
                end_line,
                end_column,
                severity: severity_str(d.severity).to_string(),
                message: d.message,
            }
        })
        .collect()
}

/// Resolve paths (files and directories) into BR files and check them all in parallel.
pub fn check_paths(paths: &[PathBuf]) -> Vec<FileDiagnostic> {
    let file_paths: Vec<PathBuf> = paths
        .iter()
        .flat_map(|p| {
            if p.is_dir() {
                WalkDir::new(p)
                    .follow_links(true)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file() && workspace::is_br_file(e.path()))
                    .map(|e| e.into_path())
                    .collect::<Vec<_>>()
            } else {
                vec![p.clone()]
            }
        })
        .collect();

    let mut results: Vec<FileDiagnostic> = file_paths
        .par_iter()
        .flat_map(|path| check_file(path))
        .collect();

    // Sort by file, then line, then column for stable output
    results.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });

    results
}

/// Escape a value for CSV output. Wraps in quotes if the value contains
/// commas, quotes, or newlines. Doubles any existing quotes.
fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        let escaped = value.replace('"', "\"\"");
        format!("\"{escaped}\"")
    } else {
        value.to_string()
    }
}

/// Format diagnostics as CSV with a header row.
pub fn format_csv(diagnostics: &[FileDiagnostic]) -> String {
    let mut out = String::from("file,line,column,end_line,end_column,severity,message\n");
    for d in diagnostics {
        out.push_str(&csv_escape(&d.file));
        out.push(',');
        out.push_str(&d.line.to_string());
        out.push(',');
        out.push_str(&d.column.to_string());
        out.push(',');
        out.push_str(&d.end_line.to_string());
        out.push(',');
        out.push_str(&d.end_column.to_string());
        out.push(',');
        out.push_str(&d.severity);
        out.push(',');
        out.push_str(&csv_escape(&d.message));
        out.push('\n');
    }
    out
}

/// Entry point for CLI `check` subcommand. Returns exit code.
pub fn run_check(args: &[String]) -> i32 {
    if args.is_empty() {
        eprintln!("Usage: br-lsp check <files-or-dirs>...");
        return 2;
    }

    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let diagnostics = check_paths(&paths);
    let csv = format_csv(&diagnostics);
    print!("{csv}");

    if diagnostics.iter().any(|d| d.severity == "error") {
        1
    } else {
        0
    }
}

/// Convert LSP diagnostics paired with URIs into CSV format.
pub fn diagnostics_to_csv(results: &[(Url, Vec<Diagnostic>)]) -> String {
    let mut file_diags: Vec<FileDiagnostic> = results
        .iter()
        .flat_map(|(uri, diags)| {
            let file = uri
                .to_file_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|_| uri.to_string());
            diags.iter().map(move |d| {
                let (line, column, end_line, end_column) = range_to_1based(&d.range);
                FileDiagnostic {
                    file: file.clone(),
                    line,
                    column,
                    end_line,
                    end_column,
                    severity: severity_str(d.severity).to_string(),
                    message: d.message.clone(),
                }
            })
        })
        .collect();
    file_diags.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then(a.line.cmp(&b.line))
            .then(a.column.cmp(&b.column))
    });
    format_csv(&file_diags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_escape_plain() {
        assert_eq!(csv_escape("hello"), "hello");
    }

    #[test]
    fn csv_escape_with_comma() {
        assert_eq!(csv_escape("hello, world"), "\"hello, world\"");
    }

    #[test]
    fn csv_escape_with_quotes() {
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn csv_escape_with_newline() {
        assert_eq!(csv_escape("line1\nline2"), "\"line1\nline2\"");
    }

    #[test]
    fn format_csv_empty() {
        let csv = format_csv(&[]);
        assert_eq!(
            csv,
            "file,line,column,end_line,end_column,severity,message\n"
        );
    }

    #[test]
    fn format_csv_one_diagnostic() {
        let diags = vec![FileDiagnostic {
            file: "test.brs".to_string(),
            line: 10,
            column: 1,
            end_line: 10,
            end_column: 15,
            severity: "error".to_string(),
            message: "Syntax error".to_string(),
        }];
        let csv = format_csv(&diags);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[0],
            "file,line,column,end_line,end_column,severity,message"
        );
        assert_eq!(lines[1], "test.brs,10,1,10,15,error,Syntax error");
    }

    #[test]
    fn format_csv_message_with_comma() {
        let diags = vec![FileDiagnostic {
            file: "test.brs".to_string(),
            line: 20,
            column: 5,
            end_line: 20,
            end_column: 20,
            severity: "warning".to_string(),
            message: "Function 'fnFoo' expects 2 parameter(s), but 1 provided".to_string(),
        }];
        let csv = format_csv(&diags);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with("\"Function 'fnFoo' expects 2 parameter(s), but 1 provided\""));
    }

    #[test]
    fn check_file_with_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.brs");
        std::fs::write(&file, b"let x = = =\n").unwrap();
        let diags = check_file(&file);
        assert!(!diags.is_empty());
        assert!(diags.iter().any(|d| d.severity == "error"));
    }

    #[test]
    fn check_file_clean() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("good.brs");
        std::fs::write(&file, b"let x = 1\n").unwrap();
        let diags = check_file(&file);
        assert!(diags.is_empty());
    }

    #[test]
    fn check_paths_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.brs"), b"let x = = =\n").unwrap();
        std::fs::write(dir.path().join("b.txt"), b"let x = = =\n").unwrap();
        std::fs::write(dir.path().join("c.wbs"), b"let y = 1\n").unwrap();

        let diags = check_paths(&[dir.path().to_path_buf()]);
        // Only .brs and .wbs checked; a.brs has errors, c.wbs is clean
        assert!(!diags.is_empty());
        assert!(diags.iter().all(|d| d.file.contains("a.brs")));
    }

    #[test]
    fn run_check_no_args() {
        assert_eq!(run_check(&[]), 2);
    }

    #[test]
    fn run_check_clean_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("clean.brs");
        std::fs::write(&file, b"let x = 1\n").unwrap();
        let code = run_check(&[file.display().to_string()]);
        assert_eq!(code, 0);
    }

    #[test]
    fn run_check_file_with_errors() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("bad.brs");
        std::fs::write(&file, b"let x = = =\n").unwrap();
        let code = run_check(&[file.display().to_string()]);
        assert_eq!(code, 1);
    }

    #[test]
    fn range_to_1based_converts() {
        use tower_lsp::lsp_types::Position;
        let range = Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 5,
                character: 10,
            },
        };
        assert_eq!(range_to_1based(&range), (1, 1, 6, 11));
    }
}
