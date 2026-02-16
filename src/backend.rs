use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use log::{debug, error, warn};
use ropey::Rope;
use serde_json::Value;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{notification, request, *};
use tower_lsp::{Client, LanguageServer};
use tree_sitter::Tree;
use rayon::prelude::*;
use walkdir::WalkDir;

use crate::extract;
use crate::parser;
use crate::references;
use crate::workspace::{self, WorkspaceIndex};

pub struct DocumentState {
    #[allow(dead_code)]
    pub rope: Rope,
    pub source: String,
    #[allow(dead_code)]
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

impl Backend {
    async fn on_change(&self, params: TextDocumentItem) {
        let rope = Rope::from_str(&params.text);

        let tree = {
            let mut parser = self.parser.lock().unwrap();
            parser::parse(&mut parser, &params.text, None)
        };

        let diagnostics = tree
            .as_ref()
            .map(|t| parser::collect_diagnostics(t, &params.text))
            .unwrap_or_default();

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

        self.client
            .publish_diagnostics(params.uri, diagnostics, None)
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
                        change: Some(TextDocumentSyncKind::FULL),
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
                let file_defs =
                    Self::scan_workspace_folder(folder, &mut total_files_scanned);
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
        self.on_change(TextDocumentItem {
            uri: params.text_document.uri,
            text: params.content_changes[0].text.clone(),
        })
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
                    let file_defs = Self::scan_workspace_folder(
                        folder,
                        &mut total_files_scanned,
                    );
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
