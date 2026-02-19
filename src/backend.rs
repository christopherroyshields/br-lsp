use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use log::{debug, error, warn};
use rayon::prelude::*;
use ropey::Rope;
use serde_json::Value;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{notification, request, *};
use tower_lsp::{Client, LanguageServer};
use tree_sitter::{InputEdit, Point, Tree};
use walkdir::WalkDir;

const DIAGNOSTICS_DEBOUNCE_MS: u64 = 150;

use crate::builtins;
use crate::check;
use crate::code_action;
use crate::completions;
use crate::definition;
use crate::diagnostics;
use crate::extract;
use crate::parser;
use crate::references;
use crate::rename;
use crate::semantic_tokens;
use crate::symbols;
use crate::workspace::{self, WorkspaceIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Br,
    Layout,
}

pub struct DocumentState {
    pub kind: DocumentKind,
    pub rope: Rope,
    pub source: String,
    pub tree: Option<Tree>,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsConfig {
    pub syntax: bool,
    pub functions: bool,
    pub undefined_functions: bool,
    pub unused_variables: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            syntax: true,
            functions: true,
            undefined_functions: true,
            unused_variables: true,
        }
    }
}

pub struct Backend {
    pub client: Client,
    pub document_map: Arc<DashMap<String, DocumentState>>,
    pub parser: Mutex<tree_sitter::Parser>,
    pub workspace_index: Arc<tokio::sync::RwLock<WorkspaceIndex>>,
    pub layout_index: Arc<tokio::sync::RwLock<crate::layout::LayoutIndex>>,
    pub workspace_folders: Arc<tokio::sync::RwLock<Vec<Url>>>,
    pub indexing_complete: Arc<AtomicBool>,
    pub diagnostics_generation: Arc<DashMap<String, Arc<AtomicU64>>>,
    pub diagnostics_config: Arc<tokio::sync::RwLock<DiagnosticsConfig>>,
}

struct TextDocumentItem {
    uri: Url,
    text: String,
    language_id: String,
}

/// Apply one incremental LSP change to the rope and source string, returning
/// the corresponding tree-sitter `InputEdit`. BR source is ASCII so byte
/// offsets equal char offsets — no UTF-16 conversion needed.
fn apply_change(rope: &mut Rope, source: &mut String, range: &Range, new_text: &str) -> InputEdit {
    let start_line = range.start.line as usize;
    let start_col = range.start.character as usize;
    let end_line = range.end.line as usize;
    let end_col = range.end.character as usize;

    let start_char = rope.line_to_char(start_line) + start_col;
    let end_char = rope.line_to_char(end_line) + end_col;

    let start_byte = start_char; // ASCII: 1 byte per char
    let old_end_byte = end_char;

    let new_end_byte = start_byte + new_text.len();

    // Compute new_end_position by scanning new_text for newlines
    let new_end_position = {
        let mut line = start_line;
        let mut col = start_col;
        for ch in new_text.chars() {
            if ch == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        Point::new(line, col)
    };

    // Mutate rope and source
    rope.remove(start_char..end_char);
    rope.insert(start_char, new_text);
    source.replace_range(start_byte..old_end_byte, new_text);

    InputEdit {
        start_byte,
        old_end_byte,
        new_end_byte,
        start_position: Point::new(start_line, start_col),
        old_end_position: Point::new(end_line, end_col),
        new_end_position,
    }
}

impl Backend {
    fn is_layout_doc(&self, uri: &str) -> bool {
        self.document_map
            .get(uri)
            .map(|d| d.kind == DocumentKind::Layout)
            .unwrap_or(false)
    }

    async fn pull_diagnostics_config(&self) {
        let items = vec![ConfigurationItem {
            scope_uri: None,
            section: Some("br-lsp.diagnostics".to_string()),
        }];

        let values = match self.client.configuration(items).await {
            Ok(v) => v,
            Err(e) => {
                warn!("Failed to pull diagnostics config: {e}");
                return;
            }
        };

        let val = match values.into_iter().next() {
            Some(v) => v,
            None => return,
        };

        let mut config = self.diagnostics_config.write().await;
        if let Some(obj) = val.as_object() {
            if let Some(v) = obj.get("syntax").and_then(|v| v.as_bool()) {
                config.syntax = v;
            }
            if let Some(v) = obj.get("functions").and_then(|v| v.as_bool()) {
                config.functions = v;
            }
            if let Some(v) = obj.get("undefinedFunctions").and_then(|v| v.as_bool()) {
                config.undefined_functions = v;
            }
            if let Some(v) = obj.get("unusedVariables").and_then(|v| v.as_bool()) {
                config.unused_variables = v;
            }
        }

        debug!("diagnostics config updated: {config:?}");
    }

    async fn republish_all_diagnostics(&self) {
        let config = self.diagnostics_config.read().await;
        let index = if self.indexing_complete.load(Ordering::Acquire) {
            Some(self.workspace_index.read().await)
        } else {
            None
        };

        let to_publish: Vec<(String, Vec<Diagnostic>)> = self
            .document_map
            .iter()
            .filter_map(|entry| {
                let uri_string = entry.key().clone();
                let doc = entry.value();
                let t = doc.tree.as_ref()?;
                let diags =
                    Self::collect_all_diagnostics(t, &doc.source, &config, index.as_deref());
                Some((uri_string, diags))
            })
            .collect();

        for (uri_string, diags) in to_publish {
            if let Ok(uri) = Url::parse(&uri_string) {
                self.client.publish_diagnostics(uri, diags, None).await;
            }
        }
    }

    fn collect_all_diagnostics(
        tree: &Tree,
        source: &str,
        config: &DiagnosticsConfig,
        index: Option<&WorkspaceIndex>,
    ) -> Vec<Diagnostic> {
        let mut diagnostics = if config.syntax {
            parser::collect_diagnostics(tree, source)
        } else {
            Vec::new()
        };

        if config.functions {
            diagnostics.extend(diagnostics::collect_function_diagnostics(tree, source));
        }

        if config.unused_variables {
            diagnostics.extend(diagnostics::check_unused_variables(tree, source));
        }

        if config.undefined_functions {
            if let Some(idx) = index {
                diagnostics.extend(diagnostics::check_undefined_functions(tree, source, idx));
            }
        }

        diagnostics
    }

    async fn on_change(&self, params: TextDocumentItem) {
        let kind = if params.language_id == "lay" {
            DocumentKind::Layout
        } else {
            DocumentKind::Br
        };

        if kind == DocumentKind::Layout {
            let rope = Rope::from_str(&params.text);
            let uri_string = params.uri.to_string();

            // Parse layout and update layout index
            if let Some(layout) = crate::layout::parse(&params.text) {
                let mut idx = self.layout_index.write().await;
                idx.update(&uri_string, layout);
            }

            self.document_map.insert(
                uri_string,
                DocumentState {
                    kind,
                    rope,
                    source: params.text,
                    tree: None,
                },
            );

            // Publish empty diagnostics for layout files
            self.client
                .publish_diagnostics(params.uri, vec![], None)
                .await;
            return;
        }

        let start = std::time::Instant::now();
        let rope = Rope::from_str(&params.text);

        let tree = {
            let mut parser = self.parser.lock().unwrap();
            parser::parse(&mut parser, &params.text, None)
        };
        let parse_elapsed = start.elapsed();

        // Update workspace index with definitions from this file
        if let Some(t) = tree.as_ref() {
            let defs = extract::extract_definitions(t, &params.text);
            let mut index = self.workspace_index.write().await;
            index.update_file(&params.uri, defs);
        }

        let diagnostics = if let Some(t) = tree.as_ref() {
            let config = self.diagnostics_config.read().await;
            let index = if self.indexing_complete.load(Ordering::Acquire) {
                Some(self.workspace_index.read().await)
            } else {
                None
            };
            Self::collect_all_diagnostics(t, &params.text, &config, index.as_deref())
        } else {
            Vec::new()
        };

        let uri_string = params.uri.to_string();
        self.document_map.insert(
            uri_string,
            DocumentState {
                kind,
                rope,
                source: params.text.clone(),
                tree,
            },
        );

        let total_elapsed = start.elapsed();

        self.client
            .publish_diagnostics(params.uri, diagnostics, None)
            .await;

        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "on_change (full parse): {} bytes, parse {parse_elapsed:.1?}, total {total_elapsed:.1?}",
                    params.text.len()
                ),
            )
            .await;
    }

    fn schedule_diagnostics(&self, uri: Url, uri_string: String) {
        let generation = self
            .diagnostics_generation
            .entry(uri_string.clone())
            .or_insert_with(|| Arc::new(AtomicU64::new(0)))
            .clone();
        let my_gen = generation.fetch_add(1, Ordering::SeqCst) + 1;

        let client = self.client.clone();
        let document_map = self.document_map.clone();
        let workspace_index = self.workspace_index.clone();
        let indexing_complete = self.indexing_complete.clone();
        let diagnostics_config = self.diagnostics_config.clone();

        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(DIAGNOSTICS_DEBOUNCE_MS)).await;

            if generation.load(Ordering::SeqCst) != my_gen {
                return; // stale — a newer change superseded us
            }

            let start = std::time::Instant::now();

            let (source, tree) = {
                let doc = match document_map.get(&uri_string) {
                    Some(d) => d,
                    None => return, // document was closed
                };
                (doc.source.clone(), doc.tree.clone())
            };

            let tree = match tree {
                Some(t) => t,
                None => return,
            };

            let defs = extract::extract_definitions(&tree, &source);
            {
                let mut index = workspace_index.write().await;
                index.update_file(&uri, defs);
            }

            let config = diagnostics_config.read().await;
            let index = if indexing_complete.load(Ordering::Acquire) {
                Some(workspace_index.read().await)
            } else {
                None
            };
            let diagnostics =
                Backend::collect_all_diagnostics(&tree, &source, &config, index.as_deref());

            let count = diagnostics.len();
            client.publish_diagnostics(uri, diagnostics, None).await;

            client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "diagnostics (debounced): {count} diagnostics, {} bytes ({:.1?})",
                        source.len(),
                        start.elapsed()
                    ),
                )
                .await;
        });
    }

    fn scan_workspace_folder(
        folder: &Url,
        files_scanned: &mut usize,
    ) -> Vec<(Url, Vec<extract::FunctionDef>)> {
        let path = match folder.to_file_path() {
            Ok(p) => p,
            Err(()) => {
                warn!("Cannot convert workspace folder URI to path: {folder}");
                return Vec::new();
            }
        };

        // Collect file paths first (walkdir is single-threaded)
        let file_paths: Vec<_> = WalkDir::new(&path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && workspace::is_br_file(e.path()))
            .map(|e| e.into_path())
            .collect();

        *files_scanned += file_paths.len();

        // Parse in parallel — each thread gets its own parser
        file_paths
            .par_iter()
            .filter_map(|file_path| {
                let source = match workspace::read_br_file(file_path) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to read {}: {e}", file_path.display());
                        return None;
                    }
                };

                let mut parser = parser::new_parser();
                let tree = parser::parse(&mut parser, &source, None)?;
                let defs = extract::extract_definitions(&tree, &source);
                if defs.is_empty() {
                    return None;
                }

                let uri = Url::from_file_path(file_path).ok()?;
                Some((uri, defs))
            })
            .collect()
    }

    /// Search all workspace files (open + closed) for references to a function name.
    async fn search_workspace_for_function_refs(&self, name: &str) -> Vec<Location> {
        let mut locations = Vec::new();

        // 1. Open documents
        let mut open_uris = std::collections::HashSet::new();
        for entry in self.document_map.iter() {
            let uri_string = entry.key().clone();
            open_uris.insert(uri_string.clone());
            if let Some(tree) = entry.value().tree.as_ref() {
                let refs =
                    references::find_function_refs_by_name(name, tree, &entry.value().source);
                if let Ok(uri) = Url::parse(&uri_string) {
                    for range in refs {
                        locations.push(Location {
                            uri: uri.clone(),
                            range,
                        });
                    }
                }
            }
        }

        // 2. Closed files — parallel walk of workspace folders
        let folders = self.workspace_folders.read().await.clone();
        let name_owned = name.to_string();
        let open_uris_clone = open_uris;

        let closed_locations = tokio::task::spawn_blocking(move || {
            let mut results = Vec::new();
            for folder in &folders {
                let path = match folder.to_file_path() {
                    Ok(p) => p,
                    Err(()) => continue,
                };

                let file_paths: Vec<_> = WalkDir::new(&path)
                    .follow_links(true)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|e| e.file_type().is_file() && workspace::is_br_file(e.path()))
                    .map(|e| e.into_path())
                    .collect();

                let folder_results: Vec<Location> = file_paths
                    .par_iter()
                    .filter_map(|file_path| {
                        let uri = Url::from_file_path(file_path).ok()?;
                        if open_uris_clone.contains(uri.as_str()) {
                            return None;
                        }
                        let source = workspace::read_br_file(file_path).ok()?;
                        let mut parser = parser::new_parser();
                        let tree = parser::parse(&mut parser, &source, None)?;
                        let refs =
                            references::find_function_refs_by_name(&name_owned, &tree, &source);
                        if refs.is_empty() {
                            return None;
                        }
                        Some(
                            refs.into_iter()
                                .map(|range| Location {
                                    uri: uri.clone(),
                                    range,
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .flatten()
                    .collect();

                results.extend(folder_results);
            }
            results
        })
        .await
        .unwrap_or_default();

        locations.extend(closed_locations);
        locations
    }

    fn scan_workspace_diagnostics(
        folder: &Url,
        config: &DiagnosticsConfig,
    ) -> Vec<(Url, Vec<Diagnostic>)> {
        let path = match folder.to_file_path() {
            Ok(p) => p,
            Err(()) => {
                warn!("Cannot convert workspace folder URI to path: {folder}");
                return Vec::new();
            }
        };

        let file_paths: Vec<_> = WalkDir::new(&path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && workspace::is_br_file(e.path()))
            .map(|e| e.into_path())
            .collect();

        file_paths
            .par_iter()
            .filter_map(|file_path| {
                let source = match workspace::read_br_file(file_path) {
                    Ok(s) => s,
                    Err(e) => {
                        warn!("Failed to read {}: {e}", file_path.display());
                        return None;
                    }
                };

                let mut ts_parser = parser::new_parser();
                let tree = parser::parse(&mut ts_parser, &source, None)?;

                let diags = Self::collect_all_diagnostics(&tree, &source, config, None);

                let uri = Url::from_file_path(file_path).ok()?;
                Some((uri, diags))
            })
            .collect()
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Capture workspace folders
        let mut folders = self.workspace_folders.write().await;
        if let Some(wf) = params.workspace_folders {
            for folder in wf {
                folders.push(folder.uri);
            }
        } else if let Some(root_uri) = params.root_uri {
            folders.push(root_uri);
        }

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "br-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
            offset_encoding: None,
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::INCREMENTAL),
                        save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                            include_text: Some(true),
                        })),
                        ..Default::default()
                    },
                )),
                completion_provider: Some(CompletionOptions {
                    resolve_provider: Some(true),
                    trigger_characters: None,
                    work_done_progress_options: Default::default(),
                    all_commit_characters: None,
                    completion_item: None,
                }),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".into(), ",".into()]),
                    retrigger_characters: None,
                    work_done_progress_options: Default::default(),
                }),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            legend: semantic_tokens::legend(),
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                            ..Default::default()
                        },
                    ),
                ),
                definition_provider: Some(OneOf::Left(true)),
                references_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
                        ..Default::default()
                    },
                )),
                document_highlight_provider: Some(OneOf::Left(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                workspace: Some(WorkspaceServerCapabilities {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(OneOf::Left(true)),
                    }),
                    file_operations: None,
                }),
                ..ServerCapabilities::default()
            },
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        debug!("initialized!");

        // Register file watcher for .brs, .wbs, .lay, and filelay/* files
        let registrations = vec![Registration {
            id: "br-file-watcher".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: vec![
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.brs".to_string()),
                            kind: Some(WatchKind::all()),
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.wbs".to_string()),
                            kind: Some(WatchKind::all()),
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/*.lay".to_string()),
                            kind: Some(WatchKind::all()),
                        },
                        FileSystemWatcher {
                            glob_pattern: GlobPattern::String("**/filelay/*".to_string()),
                            kind: Some(WatchKind::all()),
                        },
                    ],
                })
                .unwrap(),
            ),
        }];

        if let Err(e) = self.client.register_capability(registrations).await {
            warn!("Failed to register file watcher: {e}");
        }

        // Pull initial diagnostics config from the client
        self.pull_diagnostics_config().await;

        // Spawn background workspace scan
        let folders = self.workspace_folders.read().await.clone();
        let index = self.workspace_index.clone();
        let layout_index = self.layout_index.clone();
        let client = self.client.clone();
        let indexing_complete = self.indexing_complete.clone();
        let document_map = self.document_map.clone();
        let diagnostics_config = self.diagnostics_config.clone();

        tokio::spawn(async move {
            let token = NumberOrString::String("workspace-indexing".to_string());

            // Create progress token
            let _ = client
                .send_request::<request::WorkDoneProgressCreate>(WorkDoneProgressCreateParams {
                    token: token.clone(),
                })
                .await;

            // Begin progress
            client
                .send_notification::<notification::Progress>(ProgressParams {
                    token: token.clone(),
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::Begin(
                        WorkDoneProgressBegin {
                            title: "Indexing BR files".to_string(),
                            cancellable: Some(false),
                            message: Some("Scanning workspace...".to_string()),
                            percentage: None,
                        },
                    )),
                })
                .await;

            let start = std::time::Instant::now();
            let mut total = 0usize;
            let mut total_files_scanned = 0usize;

            for folder in &folders {
                let file_defs = Self::scan_workspace_folder(folder, &mut total_files_scanned);
                let count = file_defs.len();

                let mut idx = index.write().await;
                for (uri, defs) in file_defs {
                    idx.add_file(&uri, defs);
                }
                total += count;
            }

            // Scan for layout files
            let mut layout_count = 0usize;
            for folder in &folders {
                let layouts = crate::layout::scan_workspace_layouts(folder);
                layout_count += layouts.len();
                let mut lidx = layout_index.write().await;
                for (uri, layout) in layouts {
                    lidx.add(&uri, layout);
                }
            }

            let elapsed = start.elapsed();
            let summary = format!(
                "scanned {total_files_scanned} files, {total} contain definitions, {layout_count} layouts ({elapsed:.1?})"
            );

            // End progress
            client
                .send_notification::<notification::Progress>(ProgressParams {
                    token,
                    value: ProgressParamsValue::WorkDone(WorkDoneProgress::End(
                        WorkDoneProgressEnd {
                            message: Some(summary.clone()),
                        },
                    )),
                })
                .await;

            client
                .log_message(
                    MessageType::INFO,
                    format!("Workspace indexing complete: {summary}"),
                )
                .await;

            indexing_complete.store(true, Ordering::Release);

            // Re-publish diagnostics for all open documents now that the
            // workspace index is available for undefined-function checks.
            let to_publish: Vec<(String, Vec<Diagnostic>)> = {
                let config = diagnostics_config.read().await;
                let idx = index.read().await;
                document_map
                    .iter()
                    .filter_map(|entry| {
                        let uri_string = entry.key().clone();
                        let doc = entry.value();
                        let t = doc.tree.as_ref()?;
                        let diags =
                            Backend::collect_all_diagnostics(t, &doc.source, &config, Some(&idx));
                        Some((uri_string, diags))
                    })
                    .collect()
            };

            for (uri_string, diags) in to_publish {
                if let Ok(uri) = Url::parse(&uri_string) {
                    client.publish_diagnostics(uri, diags, None).await;
                }
            }
        });
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.text_document.text,
            language_id: params.text_document.language_id,
        })
        .await;
        debug!("file opened!");
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        let uri_string = uri.to_string();
        let change_count = params.content_changes.len();

        let Some(mut doc) = self.document_map.get_mut(&uri_string) else {
            // Document not in map — fall back to full parse with inferred language
            if let Some(change) = params.content_changes.into_iter().last() {
                let lang = if uri.path().ends_with(".lay") || uri.path().contains("/filelay/") {
                    "lay"
                } else {
                    "br"
                };
                self.on_change(TextDocumentItem {
                    uri,
                    text: change.text,
                    language_id: lang.to_string(),
                })
                .await;
            }
            return;
        };

        // Layout documents: just update source/rope and layout index
        if doc.kind == DocumentKind::Layout {
            let DocumentState {
                ref mut rope,
                ref mut source,
                ..
            } = *doc;
            for change in params.content_changes {
                match change.range {
                    Some(range) => {
                        apply_change(rope, source, &range, &change.text);
                    }
                    None => {
                        *rope = Rope::from_str(&change.text);
                        *source = change.text;
                    }
                }
            }

            let source = doc.source.clone();
            drop(doc);

            if let Some(layout) = crate::layout::parse(&source) {
                let mut idx = self.layout_index.write().await;
                idx.update(&uri_string, layout);
            }
            return;
        }

        // Apply each incremental change
        let DocumentState {
            ref mut rope,
            ref mut source,
            ref mut tree,
            ..
        } = *doc;

        let had_old_tree = tree.is_some();
        for change in params.content_changes {
            match change.range {
                Some(range) => {
                    let edit = apply_change(rope, source, &range, &change.text);
                    if let Some(t) = tree.as_mut() {
                        t.edit(&edit);
                    }
                }
                None => {
                    // Full replacement — reset everything
                    *rope = Rope::from_str(&change.text);
                    *source = change.text;
                    *tree = None;
                }
            }
        }
        let edit_elapsed = start.elapsed();

        // Reparse (incremental if we have an old tree)
        let incremental = doc.tree.is_some();
        let tree = {
            let mut parser = self.parser.lock().unwrap();
            parser::parse(&mut parser, &doc.source, doc.tree.as_ref())
        };
        let parse_elapsed = start.elapsed() - edit_elapsed;

        let source_len = doc.source.len();
        doc.tree = tree;

        // Drop the DashMap RefMut before awaiting (it's not Send)
        drop(doc);

        let total_elapsed = start.elapsed();

        let mode = if incremental {
            "incremental"
        } else if had_old_tree {
            "full (tree reset)"
        } else {
            "full (no prior tree)"
        };
        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "did_change ({mode}): {source_len} bytes, {change_count} change(s), edit {edit_elapsed:.1?}, parse {parse_elapsed:.1?}, total {total_elapsed:.1?}"
                ),
            )
            .await;

        // Schedule debounced diagnostics (runs after 150ms if no newer changes arrive)
        self.schedule_diagnostics(uri, uri_string);
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        debug!("file saved!");
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        let was_layout = self
            .document_map
            .get(&uri)
            .map(|d| d.kind == DocumentKind::Layout)
            .unwrap_or(false);
        self.document_map.remove(&uri);
        if was_layout {
            let mut idx = self.layout_index.write().await;
            idx.remove(&uri);
        }
        self.client
            .publish_diagnostics(params.text_document.uri, vec![], None)
            .await;
        debug!("file closed!");
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri.to_string();
        let position = params.text_document_position.position;

        if self.is_layout_doc(&uri) {
            return Ok(None);
        }

        let index = self.workspace_index.read().await;
        let layout_index = self.layout_index.read().await;
        let items = match self.document_map.get(&uri) {
            Some(doc) => completions::get_completions(&doc, &uri, position, &index, &layout_index),
            None => return Ok(None),
        };

        let count = items.len();
        let result = if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::Array(items)))
        };

        self.client
            .log_message(
                MessageType::LOG,
                format!("completion: {count} items ({:.1?})", start.elapsed()),
            )
            .await;

        result
    }

    async fn completion_resolve(&self, mut item: CompletionItem) -> Result<CompletionItem> {
        let data = match item
            .data
            .as_ref()
            .and_then(|v| serde_json::from_value::<completions::CompletionData>(v.clone()).ok())
        {
            Some(d) => d,
            None => return Ok(item),
        };

        let docs = match data {
            completions::CompletionData::Builtin { ref name, overload } => {
                let entries = builtins::lookup(name);
                entries.get(overload).map(completions::format_builtin_docs)
            }
            completions::CompletionData::Local { ref name, ref uri } => {
                self.document_map.get(uri).and_then(|doc| {
                    let tree = doc.tree.as_ref()?;
                    let defs = extract::extract_definitions(tree, &doc.source);
                    defs.into_iter()
                        .find(|d| d.name.eq_ignore_ascii_case(name))
                        .map(|d| completions::format_function_docs(&d))
                })
            }
            completions::CompletionData::Workspace { ref name } => {
                let index = self.workspace_index.read().await;
                index
                    .lookup_best(name, "")
                    .map(|e| completions::format_function_docs(&e.def))
            }
        };

        if let Some(md) = docs {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }));
        }

        Ok(item)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri.clone();
        let uri_string = uri.to_string();
        let position = params.text_document_position.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        // Check if cursor is on a user function name (cross-file candidate)
        let fn_name = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let name = references::resolve_function_name_at(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
            )?;
            if !builtins::lookup(&name).is_empty() {
                return None; // system function — stay single-file
            }
            Some(name)
        });

        if let Some(name) = fn_name {
            // Cross-file search for user function references
            let locations = self.search_workspace_for_function_refs(&name).await;
            let count = locations.len();
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "references (cross-file, \"{name}\"): {count} locations ({:.1?})",
                        start.elapsed()
                    ),
                )
                .await;
            if locations.is_empty() {
                return Ok(None);
            }
            return Ok(Some(locations));
        }

        // Non-function symbols: single-file references
        let locations = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let refs = references::find_references(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
            );
            if refs.is_empty() {
                None
            } else {
                Some(
                    refs.into_iter()
                        .map(|range| Location {
                            uri: uri.clone(),
                            range,
                        })
                        .collect(),
                )
            }
        });

        let count = locations.as_ref().map_or(0, |v: &Vec<Location>| v.len());
        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "references (local): {count} locations ({:.1?})",
                    start.elapsed()
                ),
            )
            .await;

        Ok(locations)
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let uri_string = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        let highlights = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let refs = references::find_references(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
            );
            if refs.is_empty() {
                None
            } else {
                Some(
                    refs.into_iter()
                        .map(|range| DocumentHighlight {
                            range,
                            kind: Some(DocumentHighlightKind::TEXT),
                        })
                        .collect(),
                )
            }
        });

        Ok(highlights)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri_string = params.text_document.uri.to_string();
        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }
        let result = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let r = rename::prepare_rename(
                tree,
                &doc.source,
                params.position.line as usize,
                params.position.character as usize,
            )?;
            Some(PrepareRenameResponse::RangeWithPlaceholder {
                range: r.range,
                placeholder: r.placeholder,
            })
        });
        Ok(result)
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri.clone();
        let uri_string = uri.to_string();
        let position = params.text_document_position.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        // Check if cursor is on a user function name (cross-file candidate)
        let fn_name = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let name = references::resolve_function_name_at(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
            )?;
            if !builtins::lookup(&name).is_empty() {
                return None; // system function — rejected by prepare_rename
            }
            Some(name)
        });

        if let Some(name) = fn_name {
            // Cross-file rename for user functions
            let locations = self.search_workspace_for_function_refs(&name).await;
            if locations.is_empty() {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!(
                            "rename (cross-file, \"{name}\" -> \"{}\"): 0 edits ({:.1?})",
                            params.new_name,
                            start.elapsed()
                        ),
                    )
                    .await;
                return Ok(None);
            }
            let edit_count = locations.len();
            let mut changes: std::collections::HashMap<Url, Vec<TextEdit>> =
                std::collections::HashMap::new();
            for loc in locations {
                changes.entry(loc.uri).or_default().push(TextEdit {
                    range: loc.range,
                    new_text: params.new_name.clone(),
                });
            }
            let file_count = changes.len();
            self.client
                .log_message(
                    MessageType::LOG,
                    format!(
                        "rename (cross-file, \"{name}\" -> \"{}\"): {edit_count} edits across {file_count} files ({:.1?})",
                        params.new_name,
                        start.elapsed()
                    ),
                )
                .await;
            return Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                ..Default::default()
            }));
        }

        // Non-function symbols: single-file rename
        let edits = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            let text_edits = rename::compute_renames(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
                &params.new_name,
            );
            if text_edits.is_empty() {
                None
            } else {
                Some(text_edits)
            }
        });

        let count = edits.as_ref().map_or(0, |v| v.len());
        self.client
            .log_message(
                MessageType::LOG,
                format!("rename (local): {count} edits ({:.1?})", start.elapsed()),
            )
            .await;

        match edits {
            Some(text_edits) => {
                let mut changes = std::collections::HashMap::new();
                changes.insert(uri, text_edits);
                Ok(Some(WorkspaceEdit {
                    changes: Some(changes),
                    ..Default::default()
                }))
            }
            None => Ok(None),
        }
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        let uri_string = uri.to_string();
        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }
        let doc = match self.document_map.get(&uri_string) {
            Some(d) => d,
            None => return Ok(None),
        };
        let tree = match doc.tree.as_ref() {
            Some(t) => t,
            None => return Ok(None),
        };

        let mut actions = Vec::new();
        for diag in &params.context.diagnostics {
            if let Some(action) =
                code_action::create_function_stub_action(&uri, diag, tree, &doc.source)
            {
                actions.push(CodeActionOrCommand::CodeAction(action));
            }
        }

        Ok(if actions.is_empty() {
            None
        } else {
            Some(actions)
        })
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let start = std::time::Instant::now();
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let uri_string = uri.to_string();
        let position = params.text_document_position_params.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        let result = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            Some(definition::find_definition(
                tree,
                &doc.source,
                position.line as usize,
                position.character as usize,
            ))
        });

        let response = match result {
            Some(definition::DefinitionResult::Found(range)) => {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!("definition (local): found ({:.1?})", start.elapsed()),
                    )
                    .await;
                Ok(Some(GotoDefinitionResponse::Scalar(Location {
                    uri,
                    range,
                })))
            }
            Some(definition::DefinitionResult::LookupFunction(name)) => {
                // Extract library links from the current doc's tree before awaiting locks
                let library_links = self
                    .document_map
                    .get(&uri_string)
                    .and_then(|doc| {
                        let tree = doc.tree.as_ref()?;
                        Some(extract::extract_library_links(tree, &doc.source))
                    })
                    .unwrap_or_default();

                let folders = self.workspace_folders.read().await;
                let index = self.workspace_index.read().await;
                let def = index
                    .lookup_prioritized_with_links(&name, &uri_string, &library_links, &folders)
                    .into_iter()
                    .next();
                if let Some(def) = def {
                    self.client
                        .log_message(
                            MessageType::LOG,
                            format!(
                                "definition (workspace, \"{name}\"): found ({:.1?})",
                                start.elapsed()
                            ),
                        )
                        .await;
                    Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: def.uri.clone(),
                        range: def.def.selection_range,
                    })))
                } else {
                    self.client
                        .log_message(
                            MessageType::LOG,
                            format!(
                                "definition (workspace, \"{name}\"): not found ({:.1?})",
                                start.elapsed()
                            ),
                        )
                        .await;
                    Ok(None)
                }
            }
            _ => Ok(None),
        };

        response
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let start = std::time::Instant::now();
        let uri_string = params.text_document.uri.to_string();
        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }
        let result = self.document_map.get(&uri_string).and_then(|doc| {
            let tree = doc.tree.as_ref()?;
            Some(symbols::collect_document_symbols(tree, &doc.source))
        });
        match result {
            Some(syms) if !syms.is_empty() => {
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!(
                            "document_symbol: {} symbols ({:.1?})",
                            syms.len(),
                            start.elapsed()
                        ),
                    )
                    .await;
                Ok(Some(DocumentSymbolResponse::Nested(syms)))
            }
            _ => Ok(None),
        }
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri.to_string();
        let tokens = self.document_map.get(&uri).map(|doc| match doc.kind {
            DocumentKind::Layout => crate::layout::collect_layout_tokens(&doc.source),
            DocumentKind::Br => match doc.tree.as_ref() {
                Some(tree) => semantic_tokens::collect_tokens(tree, &doc.source),
                None => Vec::new(),
            },
        });
        let result = match tokens {
            Some(t) if !t.is_empty() => {
                let count = t.len();
                self.client
                    .log_message(
                        MessageType::LOG,
                        format!("semantic_tokens: {count} tokens ({:.1?})", start.elapsed()),
                    )
                    .await;
                Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data: t,
                })))
            }
            _ => Ok(None),
        };
        result
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri_string = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        // Extract everything we need from the DashMap ref, then drop it
        enum HoverKind {
            Builtin(String),
            User(String, std::collections::HashMap<String, String>),
        }

        let (hover_kind, fn_name_range) = {
            let doc = match self.document_map.get(&uri_string) {
                Some(d) => d,
                None => return Ok(None),
            };
            let tree = match doc.tree.as_ref() {
                Some(t) => t,
                None => return Ok(None),
            };

            // Find function_name node at cursor
            let mut node = match parser::node_at_position(
                tree,
                position.line as usize,
                position.character as usize,
            ) {
                Some(n) => n,
                None => return Ok(None),
            };

            // Walk up to find a function_name node
            loop {
                if node.kind() == "function_name" {
                    break;
                }
                match node.parent() {
                    Some(p) => node = p,
                    None => return Ok(None),
                }
            }

            let fn_name = match node.utf8_text(doc.source.as_bytes()) {
                Ok(s) => s.to_string(),
                Err(_) => return Ok(None),
            };
            let fn_name_range = parser::node_range(node);

            let parent = match node.parent() {
                Some(p) => p,
                None => return Ok(None),
            };

            let kind = match parent.kind() {
                "numeric_system_function" | "string_system_function" => {
                    HoverKind::Builtin(fn_name)
                }
                _ => {
                    let library_links = extract::extract_library_links(tree, &doc.source);
                    HoverKind::User(fn_name, library_links)
                }
            };

            (kind, fn_name_range)
        }; // doc dropped here

        let markdown = match hover_kind {
            HoverKind::Builtin(ref fn_name) => {
                let builtins = builtins::lookup(fn_name);
                if builtins.is_empty() {
                    return Ok(None);
                }
                format_builtin_hover(builtins)
            }
            HoverKind::User(ref fn_name, ref library_links) => {
                let folders = self.workspace_folders.read().await;
                let index = self.workspace_index.read().await;
                let defs = index.lookup_prioritized_with_links(
                    fn_name,
                    &uri_string,
                    library_links,
                    &folders,
                );
                if defs.is_empty() {
                    return Ok(None);
                }
                format_user_hover_multi(&defs)
            }
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: markdown,
            }),
            range: Some(fn_name_range),
        }))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri_string = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

        if self.is_layout_doc(&uri_string) {
            return Ok(None);
        }

        let doc = match self.document_map.get(&uri_string) {
            Some(d) => d,
            None => return Ok(None),
        };

        // Try tree-based approach first
        let call_ctx = doc
            .tree
            .as_ref()
            .and_then(|tree| {
                let cursor_node = parser::node_at_position(
                    tree,
                    position.line as usize,
                    position.character as usize,
                )?;

                // Walk up to find an arguments node
                let mut node = cursor_node;
                let args_node = loop {
                    if node.kind() == "arguments" {
                        break node;
                    }
                    node = node.parent()?;
                };

                let call_node = args_node.parent()?;

                let mut cursor = call_node.walk();
                let fn_name_node = call_node
                    .children(&mut cursor)
                    .find(|c| c.kind() == "function_name")?;

                let fn_name = fn_name_node.utf8_text(doc.source.as_bytes()).ok()?;

                // Count commas before cursor to determine active parameter
                let mut count = 0u32;
                let mut cursor = args_node.walk();
                for child in args_node.children(&mut cursor) {
                    if !child.is_named()
                        && child.utf8_text(doc.source.as_bytes()).ok() == Some(",")
                        && child.end_position().column as u32 <= position.character
                        && child.end_position().row as u32 <= position.line
                    {
                        count += 1;
                    }
                }

                Some(parser::CallContext {
                    name: fn_name.to_string(),
                    active_param: count,
                })
            })
            // Fall back to text-based scanning when tree walk fails
            .or_else(|| {
                parser::find_function_call_context(
                    &doc.source,
                    position.line as usize,
                    position.character as usize,
                )
            });

        let call_ctx = match call_ctx {
            Some(ctx) => ctx,
            None => return Ok(None),
        };

        // Extract library links before dropping the DashMap ref
        let library_links = doc
            .tree
            .as_ref()
            .map(|tree| extract::extract_library_links(tree, &doc.source))
            .unwrap_or_default();
        drop(doc);

        let signatures = {
            let builtins = builtins::lookup(&call_ctx.name);
            if !builtins.is_empty() {
                build_builtin_signatures(builtins, call_ctx.active_param)
            } else {
                let folders = self.workspace_folders.read().await;
                let index = self.workspace_index.read().await;
                match index
                    .lookup_prioritized_with_links(
                        &call_ctx.name,
                        &uri_string,
                        &library_links,
                        &folders,
                    )
                    .into_iter()
                    .next()
                {
                    Some(d) => build_user_signatures(&d.def, call_ctx.active_param),
                    None => return Ok(None),
                }
            }
        };

        Ok(Some(SignatureHelp {
            signatures,
            active_signature: Some(0),
            active_parameter: Some(call_ctx.active_param),
        }))
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        debug!("configuration changed!");
        self.pull_diagnostics_config().await;
        self.republish_all_diagnostics().await;
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        debug!("workspace folders changed!");

        let event = params.event;

        // Remove files from removed folders
        if !event.removed.is_empty() {
            let mut folders = self.workspace_folders.write().await;
            let mut index = self.workspace_index.write().await;

            for removed in &event.removed {
                folders.retain(|f| f != &removed.uri);

                // Remove all indexed definitions under this folder
                let folder_str = removed.uri.as_str();
                let to_remove: Vec<Url> = index
                    .all_symbols()
                    .iter()
                    .filter(|s| s.uri.as_str().starts_with(folder_str))
                    .map(|s| s.uri.clone())
                    .collect();

                for uri in to_remove {
                    index.remove_file(&uri);
                }
            }
        }

        // Scan added folders
        if !event.added.is_empty() {
            let new_folders: Vec<Url> = event.added.iter().map(|f| f.uri.clone()).collect();

            {
                let mut folders = self.workspace_folders.write().await;
                folders.extend(new_folders.clone());
            }

            let index = self.workspace_index.clone();
            let client = self.client.clone();

            tokio::spawn(async move {
                let start = std::time::Instant::now();
                let mut total = 0usize;
                let mut total_files_scanned = 0usize;

                for folder in &new_folders {
                    let file_defs = Self::scan_workspace_folder(folder, &mut total_files_scanned);
                    let count = file_defs.len();

                    let mut idx = index.write().await;
                    for (uri, defs) in file_defs {
                        idx.add_file(&uri, defs);
                    }
                    total += count;
                }

                let elapsed = start.elapsed();
                client
                    .log_message(
                        MessageType::INFO,
                        format!(
                            "Workspace folder scan complete in {elapsed:.1?}: scanned {total_files_scanned} files, {total} contain definitions"
                        ),
                    )
                    .await;
            });
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        debug!("watched files have changed!");

        for change in params.changes {
            let file_path = match change.uri.to_file_path() {
                Ok(p) => p,
                Err(()) => continue,
            };

            let is_layout = crate::layout::is_layout_file(&file_path);

            match change.typ {
                FileChangeType::DELETED => {
                    if is_layout {
                        let mut idx = self.layout_index.write().await;
                        idx.remove(change.uri.as_ref());
                    } else {
                        let mut index = self.workspace_index.write().await;
                        index.remove_file(&change.uri);
                    }
                }
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    // Skip if the file is currently open — editor content takes precedence
                    if self.document_map.contains_key(change.uri.as_str()) {
                        continue;
                    }

                    if is_layout {
                        let source = match crate::layout::read_layout_file(&file_path) {
                            Ok(s) => s,
                            Err(e) => {
                                error!("Failed to read layout {}: {e}", file_path.display());
                                continue;
                            }
                        };
                        if let Some(layout) = crate::layout::parse(&source) {
                            let mut idx = self.layout_index.write().await;
                            idx.update(change.uri.as_ref(), layout);
                        }
                    } else {
                        let source = match workspace::read_br_file(&file_path) {
                            Ok(s) => s,
                            Err(e) => {
                                error!("Failed to read {}: {e}", file_path.display());
                                continue;
                            }
                        };

                        let tree = {
                            let mut parser = self.parser.lock().unwrap();
                            parser::parse(&mut parser, &source, None)
                        };

                        if let Some(t) = tree {
                            let defs = extract::extract_definitions(&t, &source);
                            let mut index = self.workspace_index.write().await;
                            index.update_file(&change.uri, defs);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
        let start = std::time::Instant::now();
        let index = self.workspace_index.read().await;
        let query = params.query.to_ascii_lowercase();

        let symbols: Vec<SymbolInformation> = index
            .all_symbols()
            .into_iter()
            .filter(|s| query.is_empty() || s.def.name.to_ascii_lowercase().contains(&query))
            .map(|s| {
                #[allow(deprecated)]
                SymbolInformation {
                    name: s.def.name.clone(),
                    kind: SymbolKind::FUNCTION,
                    tags: None,
                    deprecated: None,
                    location: Location {
                        uri: s.uri.clone(),
                        range: s.def.selection_range,
                    },
                    container_name: None,
                }
            })
            .collect();

        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "workspace_symbol (\"{}\"): {} symbols ({:.1?})",
                    params.query,
                    symbols.len(),
                    start.elapsed()
                ),
            )
            .await;

        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(symbols))
        }
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<Value>> {
        debug!("command executed: {}", params.command);

        if params.command == "br-lsp.scanAll" {
            let start = std::time::Instant::now();
            let folders = self.workspace_folders.read().await.clone();
            let config = self.diagnostics_config.read().await.clone();

            let results = tokio::task::spawn_blocking(move || {
                let mut all_results: Vec<(Url, Vec<Diagnostic>)> = Vec::new();
                for folder in &folders {
                    all_results.extend(Self::scan_workspace_diagnostics(folder, &config));
                }
                all_results
            })
            .await
            .unwrap_or_default();

            for (uri, diags) in &results {
                self.client
                    .publish_diagnostics(uri.clone(), diags.clone(), None)
                    .await;
            }

            let total_files = results.len();
            let files_with_errors = results.iter().filter(|(_, d)| !d.is_empty()).count();
            let total_diags: usize = results.iter().map(|(_, d)| d.len()).sum();
            let summary = format!("Scanned {total_files} files, {files_with_errors} with errors");

            self.client
                .log_message(
                    MessageType::INFO,
                    format!(
                        "scanAll: {total_files} files, {total_diags} diagnostics, {files_with_errors} files with errors ({:.1?})",
                        start.elapsed()
                    ),
                )
                .await;

            let csv = check::diagnostics_to_csv(&results);

            return Ok(Some(serde_json::json!({
                "summary": summary,
                "csv": csv,
            })));
        }

        Ok(None)
    }
}

fn build_builtin_signatures(
    builtins: &[builtins::BuiltinFunction],
    active_param: u32,
) -> Vec<SignatureInformation> {
    builtins
        .iter()
        .map(|b| {
            let (label, offsets) = b.format_signature_with_offsets();
            let parameters: Vec<ParameterInformation> = b
                .params
                .iter()
                .zip(offsets.iter())
                .map(|(p, off)| ParameterInformation {
                    label: ParameterLabel::LabelOffsets(*off),
                    documentation: p.documentation.as_ref().map(|d| {
                        Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: d.clone(),
                        })
                    }),
                })
                .collect();
            SignatureInformation {
                label,
                documentation: b.documentation.as_ref().map(|d| {
                    Documentation::MarkupContent(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: d.clone(),
                    })
                }),
                parameters: Some(parameters),
                active_parameter: Some(active_param),
            }
        })
        .collect()
}

fn build_user_signatures(
    def: &extract::FunctionDef,
    active_param: u32,
) -> Vec<SignatureInformation> {
    let (label, offsets) = def.format_signature_with_offsets();
    let parameters: Vec<ParameterInformation> = def
        .params
        .iter()
        .zip(offsets.iter())
        .map(|(p, off)| ParameterInformation {
            label: ParameterLabel::LabelOffsets(*off),
            documentation: p.documentation.as_ref().map(|d| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: d.clone(),
                })
            }),
        })
        .collect();
    vec![SignatureInformation {
        label,
        documentation: def.documentation.as_ref().map(|d| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: d.clone(),
            })
        }),
        parameters: Some(parameters),
        active_parameter: Some(active_param),
    }]
}

fn format_builtin_hover(builtins: &[builtins::BuiltinFunction]) -> String {
    let mut parts = Vec::new();
    for b in builtins {
        let sig = b.format_signature();
        let mut md = format!("```br\n{sig}\n```");
        if let Some(doc) = &b.documentation {
            md.push_str("\n\n---\n\n");
            md.push_str(doc);
        }
        if !b.params.is_empty() {
            let param_docs: Vec<String> = b
                .params
                .iter()
                .filter(|p| p.documentation.is_some())
                .map(|p| {
                    format!(
                        "*@param* `{}` \u{2014} {}",
                        p.name,
                        p.documentation.as_deref().unwrap()
                    )
                })
                .collect();
            if !param_docs.is_empty() {
                md.push_str("\n\n");
                md.push_str(&param_docs.join("\n\n"));
            }
        }
        parts.push(md);
    }
    parts.join("\n\n---\n\n")
}

fn format_user_hover_multi(defs: &[&workspace::IndexedFunctionDef]) -> String {
    // Filter out import-only, deduplicate by signature string
    let mut seen = std::collections::HashSet::new();
    let unique: Vec<&&workspace::IndexedFunctionDef> = defs
        .iter()
        .filter(|d| !d.def.is_import_only)
        .filter(|d| seen.insert(d.def.format_signature()))
        .collect();

    match unique.len() {
        0 => {
            // All entries are import-only; show the first one
            match defs.first() {
                Some(d) => format_user_hover(&d.def),
                None => String::new(),
            }
        }
        1 => format_user_hover(&unique[0].def),
        _ => unique
            .iter()
            .map(|d| {
                let mut md = format_user_hover(&d.def);
                let filename = uri_filename(&d.uri);
                md.push_str(&format!("\n\n*from* `{filename}`"));
                md
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
    }
}

/// Extract the filename from a URI (e.g. "file:///path/to/foo.brs" → "foo.brs").
fn uri_filename(uri: &Url) -> String {
    uri.path()
        .rsplit('/')
        .next()
        .unwrap_or(uri.as_str())
        .to_string()
}

fn format_user_hover(def: &extract::FunctionDef) -> String {
    let sig = def.format_signature();
    let mut md = format!("```br\n{sig}\n```");

    if let Some(doc) = &def.documentation {
        md.push_str("\n\n---\n\n");
        md.push_str(doc);
    }

    let param_docs: Vec<String> = def
        .params
        .iter()
        .filter(|p| p.documentation.is_some())
        .map(|p| {
            format!(
                "*@param* `{}` \u{2014} {}",
                p.format_label(),
                p.documentation.as_deref().unwrap()
            )
        })
        .collect();
    if !param_docs.is_empty() {
        md.push_str("\n\n");
        md.push_str(&param_docs.join("\n\n"));
    }

    if let Some(ret) = &def.return_documentation {
        md.push_str("\n\n");
        md.push_str(&format!("*@returns* \u{2014} {ret}"));
    }

    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_change_single_char_replacement() {
        let original = "let x = 1\n";
        let mut rope = Rope::from_str(original);
        let mut source = original.to_string();

        // Replace '1' (at line 0, col 8) with '2'
        let range = Range {
            start: Position {
                line: 0,
                character: 8,
            },
            end: Position {
                line: 0,
                character: 9,
            },
        };
        let edit = apply_change(&mut rope, &mut source, &range, "2");

        assert_eq!(source, "let x = 2\n");
        assert_eq!(rope.to_string(), "let x = 2\n");

        assert_eq!(edit.start_byte, 8);
        assert_eq!(edit.old_end_byte, 9);
        assert_eq!(edit.new_end_byte, 9);
        assert_eq!(edit.start_position, Point::new(0, 8));
        assert_eq!(edit.old_end_position, Point::new(0, 9));
        assert_eq!(edit.new_end_position, Point::new(0, 9));
    }

    #[test]
    fn apply_change_insert_newline() {
        let original = "let x = 1\n";
        let mut rope = Rope::from_str(original);
        let mut source = original.to_string();

        // Insert a newline after '1' (before the existing '\n')
        let range = Range {
            start: Position {
                line: 0,
                character: 9,
            },
            end: Position {
                line: 0,
                character: 9,
            },
        };
        let edit = apply_change(&mut rope, &mut source, &range, "\nlet y = 2");

        assert_eq!(source, "let x = 1\nlet y = 2\n");
        assert_eq!(rope.to_string(), "let x = 1\nlet y = 2\n");

        assert_eq!(edit.start_byte, 9);
        assert_eq!(edit.old_end_byte, 9); // pure insert
        assert_eq!(edit.new_end_byte, 19);
        assert_eq!(edit.new_end_position, Point::new(1, 9));
    }

    #[test]
    fn incremental_parse_matches_full_parse() {
        let original = "let x = 1\n";
        let mut parser = parser::new_parser();
        let tree = parser::parse(&mut parser, original, None).unwrap();

        // Apply an edit: replace '1' with '42'
        let mut rope = Rope::from_str(original);
        let mut source = original.to_string();
        let range = Range {
            start: Position {
                line: 0,
                character: 8,
            },
            end: Position {
                line: 0,
                character: 9,
            },
        };
        let edit = apply_change(&mut rope, &mut source, &range, "42");

        // Incremental reparse
        let mut edited_tree = tree;
        edited_tree.edit(&edit);
        let incremental = parser::parse(&mut parser, &source, Some(&edited_tree)).unwrap();

        // Full reparse from scratch
        let full = parser::parse(&mut parser, &source, None).unwrap();

        assert_eq!(
            incremental.root_node().to_sexp(),
            full.root_node().to_sexp()
        );
    }
}
