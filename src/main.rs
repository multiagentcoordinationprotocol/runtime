mod server;

use macp_runtime::log_store::LogStore;
use macp_runtime::mode_registry::ModeRegistry;
use macp_runtime::pb;
use macp_runtime::registry::SessionRegistry;
use macp_runtime::replay::replay_session;
use macp_runtime::runtime::Runtime;
use macp_runtime::security::SecurityLayer;
use macp_runtime::storage::{
    cleanup_temp_files, migrate_if_needed, FileBackend, MemoryBackend, StorageBackend,
};
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

    let storage: Arc<dyn StorageBackend> = if memory_only {
        Arc::new(MemoryBackend)
    } else {
        std::fs::create_dir_all(&data_dir)?;
        migrate_if_needed(&data_dir)?;
        cleanup_temp_files(&data_dir);
        Arc::new(FileBackend::new(data_dir.clone())?)
    };

    // Load persisted state into in-memory caches
    let registry = Arc::new(SessionRegistry::new());
    let log_store = Arc::new(LogStore::new());
    let mode_registry = Arc::new(ModeRegistry::build_default());

    if !memory_only {
        // Enumerate session directories and replay from logs
        let sessions_dir = data_dir.join("sessions");
        let mut recovered = 0usize;
        if sessions_dir.exists() {
            for entry in std::fs::read_dir(&sessions_dir)? {
                let entry = entry?;
                if !entry.file_type()?.is_dir() {
                    continue;
                }
                let session_id = entry.file_name().to_string_lossy().to_string();
                let log_entries = storage.load_log(&session_id).await?;
                if log_entries.is_empty() {
                    continue;
                }

                match replay_session(&session_id, &log_entries, &mode_registry) {
                    Ok(session) => {
                        // Best-effort snapshot update
                        if let Err(e) = storage.save_session(&session).await {
                            eprintln!(
                                "warning: failed to persist recovered session '{}': {e}",
                                session_id
                            );
                        }

                        // Populate in-memory log store
                        log_store.create_session_log(&session_id).await;
                        for log_entry in &log_entries {
                            log_store.append(&session_id, log_entry.clone()).await;
                        }

                        registry.insert_recovered_session(session_id, session).await;
                        recovered += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "warning: failed to replay session '{}': {e}; skipping",
                            session_id
                        );
                    }
                }
            }
        }
        if recovered > 0 {
            println!("Replayed {} sessions from log.", recovered);
        }
    }

    let runtime = Arc::new(Runtime::with_mode_registry(
        Arc::clone(&storage),
        Arc::clone(&registry),
        Arc::clone(&log_store),
        mode_registry,
    ));
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

    // Final snapshot: persist all sessions to storage
    if !memory_only {
        for session in registry.get_all_sessions().await {
            if let Err(e) = storage.save_session(&session).await {
                eprintln!(
                    "warning: failed to persist session '{}' on shutdown: {e}",
                    session.session_id
                );
            }
        }
    }
    println!("State persisted. Goodbye.");

    Ok(())
}
