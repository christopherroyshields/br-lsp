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

use crate::builtins;
use crate::diagnostics;
use crate::extract;
use crate::parser;
use crate::references;
use crate::workspace::{self, WorkspaceIndex};

pub struct DocumentState {
    pub rope: Rope,
    pub source: String,
    pub tree: Option<Tree>,
}

pub struct Backend {
    pub client: Client,
    pub document_map: DashMap<String, DocumentState>,
    pub parser: Mutex<tree_sitter::Parser>,
    pub workspace_index: Arc<tokio::sync::RwLock<WorkspaceIndex>>,
    pub workspace_folders: Arc<tokio::sync::RwLock<Vec<Url>>>,
}

struct TextDocumentItem {
    uri: Url,
    text: String,
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
    async fn on_change(&self, params: TextDocumentItem) {
        let start = std::time::Instant::now();
        let rope = Rope::from_str(&params.text);

        let tree = {
            let mut parser = self.parser.lock().unwrap();
            parser::parse(&mut parser, &params.text, None)
        };
        let parse_elapsed = start.elapsed();

        let mut diagnostics = tree
            .as_ref()
            .map(|t| parser::collect_diagnostics(t, &params.text))
            .unwrap_or_default();

        if let Some(t) = tree.as_ref() {
            diagnostics.extend(diagnostics::collect_function_diagnostics(t, &params.text));
        }

        // Update workspace index with definitions from this file
        if let Some(t) = tree.as_ref() {
            let defs = extract::extract_definitions(t, &params.text);
            let mut index = self.workspace_index.write().await;
            index.update_file(&params.uri, defs);
        }

        let uri_string = params.uri.to_string();
        self.document_map.insert(
            uri_string,
            DocumentState {
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
                    resolve_provider: Some(false),
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
                references_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
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

        // Register file watcher for .brs and .wbs files
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
                    ],
                })
                .unwrap(),
            ),
        }];

        if let Err(e) = self.client.register_capability(registrations).await {
            warn!("Failed to register file watcher: {e}");
        }

        // Spawn background workspace scan
        let folders = self.workspace_folders.read().await.clone();
        let index = self.workspace_index.clone();
        let client = self.client.clone();

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

            let elapsed = start.elapsed();
            let summary = format!(
                "scanned {total_files_scanned} files, {total} contain definitions ({elapsed:.1?})"
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
        });
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.text_document.text,
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
            // Document not in map — fall back to full parse
            if let Some(change) = params.content_changes.into_iter().last() {
                self.on_change(TextDocumentItem {
                    uri,
                    text: change.text,
                })
                .await;
            }
            return;
        };

        // Apply each incremental change
        let DocumentState {
            ref mut rope,
            ref mut source,
            ref mut tree,
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

        let mut diagnostics = tree
            .as_ref()
            .map(|t| parser::collect_diagnostics(t, &doc.source))
            .unwrap_or_default();

        if let Some(t) = tree.as_ref() {
            diagnostics.extend(diagnostics::collect_function_diagnostics(t, &doc.source));
        }

        let defs = tree
            .as_ref()
            .map(|t| extract::extract_definitions(t, &doc.source));

        let source_len = doc.source.len();
        doc.tree = tree;

        // Drop the DashMap RefMut before awaiting (it's not Send)
        drop(doc);

        if let Some(defs) = defs {
            let mut index = self.workspace_index.write().await;
            index.update_file(&uri, defs);
        }

        let total_elapsed = start.elapsed();

        self.client
            .publish_diagnostics(uri, diagnostics, None)
            .await;

        let mode = if incremental { "incremental" } else if had_old_tree { "full (tree reset)" } else { "full (no prior tree)" };
        self.client
            .log_message(
                MessageType::LOG,
                format!(
                    "did_change ({mode}): {source_len} bytes, {change_count} change(s), edit {edit_elapsed:.1?}, parse {parse_elapsed:.1?}, total {total_elapsed:.1?}"
                ),
            )
            .await;
    }

    async fn did_save(&self, _params: DidSaveTextDocumentParams) {
        debug!("file saved!");
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri.to_string();
        self.document_map.remove(&uri);
        self.client
            .publish_diagnostics(params.text_document.uri, vec![], None)
            .await;
        debug!("file closed!");
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri.to_string();
        debug!("completion requested for {}", uri);
        Ok(None)
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri.clone();
        let uri_string = uri.to_string();
        let position = params.text_document_position.position;
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

        Ok(locations)
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri_string = params
            .text_document_position_params
            .text_document
            .uri
            .to_string();
        let position = params.text_document_position_params.position;

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
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let fn_name_range = parser::node_range(node);

        // Check parent to determine if system or user function
        let parent = match node.parent() {
            Some(p) => p,
            None => return Ok(None),
        };

        let markdown = match parent.kind() {
            "numeric_system_function" | "string_system_function" => {
                let builtins = builtins::lookup(fn_name);
                if builtins.is_empty() {
                    return Ok(None);
                }
                format_builtin_hover(builtins)
            }
            _ => {
                // User function — look up in workspace index
                let index = self.workspace_index.read().await;
                let defs = index.lookup(fn_name);
                if defs.is_empty() {
                    return Ok(None);
                }
                format_user_hover(&defs[0].def)
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

        let signatures = {
            let builtins = builtins::lookup(&call_ctx.name);
            if !builtins.is_empty() {
                build_builtin_signatures(builtins, call_ctx.active_param)
            } else {
                let index = self.workspace_index.read().await;
                let defs = index.lookup(&call_ctx.name);
                if defs.is_empty() {
                    return Ok(None);
                }
                build_user_signatures(&defs[0].def, call_ctx.active_param)
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
            match change.typ {
                FileChangeType::DELETED => {
                    let mut index = self.workspace_index.write().await;
                    index.remove_file(&change.uri);
                }
                FileChangeType::CREATED | FileChangeType::CHANGED => {
                    // Skip if the file is currently open — editor content takes precedence
                    if self.document_map.contains_key(&change.uri.to_string()) {
                        continue;
                    }

                    let file_path = match change.uri.to_file_path() {
                        Ok(p) => p,
                        Err(()) => continue,
                    };

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
                _ => {}
            }
        }
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>> {
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

        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(symbols))
        }
    }

    async fn execute_command(&self, _: ExecuteCommandParams) -> Result<Option<Value>> {
        debug!("command executed!");
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
