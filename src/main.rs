mod server;

use macp_runtime::log_store::LogStore;
use macp_runtime::pb;
use macp_runtime::registry::SessionRegistry;
use macp_runtime::runtime::Runtime;
use macp_runtime::security::SecurityLayer;
use server::MacpServer;
use std::path::PathBuf;
use std::sync::Arc;
use tonic::transport::{Identity, Server, ServerTlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = std::env::var("MACP_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:50051".into())
        .parse()?;

    let memory_only = std::env::var("MACP_MEMORY_ONLY").ok().as_deref() == Some("1");
    let data_dir =
        PathBuf::from(std::env::var("MACP_DATA_DIR").unwrap_or_else(|_| ".macp-data".into()));

    let registry = Arc::new(if memory_only {
        SessionRegistry::new()
    } else {
        SessionRegistry::with_persistence(&data_dir)?
    });
    let log_store = Arc::new(if memory_only {
        LogStore::new()
    } else {
        LogStore::with_persistence(&data_dir)?
    });

    let registry_ref = Arc::clone(&registry);
    let log_store_ref = Arc::clone(&log_store);

    let runtime = Arc::new(Runtime::new(registry, log_store));
    let security = SecurityLayer::from_env()?;
    let svc = MacpServer::new(runtime, security);

    let allow_insecure = std::env::var("MACP_ALLOW_INSECURE").ok().as_deref() == Some("1");
    let tls_cert = std::env::var("MACP_TLS_CERT_PATH").ok();
    let tls_key = std::env::var("MACP_TLS_KEY_PATH").ok();

    println!("macp-runtime v0.4.0 listening on {}", addr);

    let builder = Server::builder();
    let mut builder = match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let cert = tokio::fs::read(cert_path).await?;
            let key = tokio::fs::read(key_path).await?;
            builder.tls_config(ServerTlsConfig::new().identity(Identity::from_pem(cert, key)))?
        }
        _ if allow_insecure => {
            eprintln!(
                "warning: starting without TLS because MACP_ALLOW_INSECURE=1; this is not RFC-compliant"
            );
            builder
        }
        _ => {
            return Err(
                "TLS is required unless MACP_ALLOW_INSECURE=1 and MACP_ALLOW_DEV_SENDER_HEADER=1"
                    .into(),
            )
        }
    };

    let server_future = builder
        .add_service(pb::macp_runtime_service_server::MacpRuntimeServiceServer::new(svc))
        .serve(addr);

    tokio::select! {
        result = server_future => {
            result?;
        }
        _ = tokio::signal::ctrl_c() => {
            println!("\nShutting down gracefully...");
        }
    }

    // Persist final state on shutdown
    if let Err(e) = registry_ref.persist_snapshot().await {
        eprintln!("warning: failed to persist session registry: {}", e);
    }
    if let Err(e) = log_store_ref.persist_snapshot().await {
        eprintln!("warning: failed to persist log store: {}", e);
    }
    println!("State persisted. Goodbye.");

    Ok(())
}
