mod backend;
mod builtins;
mod extract;
mod parser;
mod references;
mod workspace;

use std::sync::Arc;

use backend::Backend;
use dashmap::DashMap;
use tokio::sync::RwLock;
use tower_lsp::{LspService, Server};
use workspace::WorkspaceIndex;

#[tokio::main]
async fn main() {
    env_logger::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        document_map: DashMap::new(),
        parser: std::sync::Mutex::new(parser::new_parser()),
        workspace_index: Arc::new(RwLock::new(WorkspaceIndex::new())),
        workspace_folders: Arc::new(RwLock::new(Vec::new())),
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}
