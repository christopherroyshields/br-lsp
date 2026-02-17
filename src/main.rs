mod backend;
mod builtins;
mod check;
mod code_action;
mod definition;
mod diagnostics;
mod extract;
mod parser;
mod references;
mod rename;
mod semantic_tokens;
mod symbols;
mod workspace;

use std::sync::Arc;

use backend::Backend;
use dashmap::DashMap;
use tokio::sync::RwLock;
use tower_lsp::{LspService, Server};
use workspace::WorkspaceIndex;

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(|s| s.as_str()) {
        Some("check") => {
            let code = check::run_check(&args[2..]);
            std::process::exit(code);
        }
        Some("--help" | "-h") => {
            print_usage();
        }
        Some("--version" | "-V") => {
            println!("br-lsp {}", env!("CARGO_PKG_VERSION"));
        }
        _ => {
            run_lsp();
        }
    }
}

fn print_usage() {
    println!("br-lsp {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Usage:");
    println!("  br-lsp                         Start LSP server (stdin/stdout)");
    println!("  br-lsp check <files-or-dirs>   Check BR files and output diagnostics as CSV");
    println!("  br-lsp --help                  Show this help");
    println!("  br-lsp --version               Show version");
}

#[tokio::main]
async fn run_lsp() {
    env_logger::init();

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::build(|client| Backend {
        client,
        document_map: Arc::new(DashMap::new()),
        parser: std::sync::Mutex::new(parser::new_parser()),
        workspace_index: Arc::new(RwLock::new(WorkspaceIndex::new())),
        workspace_folders: Arc::new(RwLock::new(Vec::new())),
        indexing_complete: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    })
    .finish();

    Server::new(stdin, stdout, socket).serve(service).await;
}
