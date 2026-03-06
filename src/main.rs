mod server;

use macp_runtime::log_store::LogStore;
use macp_runtime::registry::SessionRegistry;
use macp_runtime::runtime::Runtime;
use server::MacpServer;
use std::sync::Arc;
use tonic::transport::Server;

use macp_runtime::pb;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:50051".parse()?;
    let registry = Arc::new(SessionRegistry::new());
    let log_store = Arc::new(LogStore::new());
    let runtime = Arc::new(Runtime::new(registry, log_store));
    let svc = MacpServer::new(runtime);

    println!("macp-runtime v0.2 (RFC-0001) listening on {}", addr);

    Server::builder()
        .add_service(pb::macp_runtime_service_server::MacpRuntimeServiceServer::new(svc))
        .serve(addr)
        .await?;

    Ok(())
}
