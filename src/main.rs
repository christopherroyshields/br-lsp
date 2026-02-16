mod backend;
mod parser;
mod references;

use backend::Backend;
use dashmap::DashMap;
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    env_logger::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        document_map: DashMap::new(),
        parser: std::sync::Mutex::new(parser::new_parser()),
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}
