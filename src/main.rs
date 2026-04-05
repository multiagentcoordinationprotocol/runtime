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
use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use tonic::transport::{Identity, Server, ServerTlsConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    #[cfg(feature = "otel")]
    {
        use opentelemetry::trace::TracerProvider;
        use opentelemetry_otlp::WithExportConfig;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;

        let otlp_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4317".into());
        let exporter = opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(&otlp_endpoint);
        let provider = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(exporter)
            .install_batch(opentelemetry_sdk::runtime::Tokio)
            .expect("failed to init OTEL tracer");
        let tracer = provider.tracer("macp-runtime");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(otel_layer)
            .init();
        tracing::info!(
            "OpenTelemetry tracing enabled (endpoint: {})",
            otlp_endpoint
        );
    }

    #[cfg(not(feature = "otel"))]
    {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    }

    let addr = std::env::var("MACP_BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:50051".into())
        .parse()?;

    let memory_only = std::env::var("MACP_MEMORY_ONLY").ok().as_deref() == Some("1");
    let data_dir =
        PathBuf::from(std::env::var("MACP_DATA_DIR").unwrap_or_else(|_| ".macp-data".into()));
    let strict_recovery = std::env::var("MACP_STRICT_RECOVERY").ok().as_deref() == Some("1");

    let backend_name = std::env::var("MACP_STORAGE_BACKEND").unwrap_or_else(|_| "file".into());
    let storage: Arc<dyn StorageBackend> = if memory_only {
        Arc::new(MemoryBackend)
    } else {
        match backend_name.as_str() {
            "file" => {
                std::fs::create_dir_all(&data_dir)?;
                migrate_if_needed(&data_dir)?;
                cleanup_temp_files(&data_dir);
                Arc::new(FileBackend::new(data_dir.clone())?)
            }
            #[cfg(feature = "rocksdb-backend")]
            "rocksdb" => {
                let path = std::env::var("MACP_ROCKSDB_PATH")
                    .unwrap_or_else(|_| data_dir.join("rocksdb").to_string_lossy().to_string());
                Arc::new(macp_runtime::storage::RocksDbBackend::open(&path)?)
            }
            #[cfg(feature = "redis-backend")]
            "redis" => {
                let url = std::env::var("MACP_REDIS_URL")
                    .unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
                Arc::new(macp_runtime::storage::RedisBackend::connect(&url, "macp").await?)
            }
            other => {
                return Err(format!(
                    "unknown storage backend: {other}. Valid: file, rocksdb, redis"
                )
                .into());
            }
        }
    };

    // Load persisted state into in-memory caches
    let registry = Arc::new(SessionRegistry::new());
    let log_store = Arc::new(LogStore::new());
    let mode_registry = Arc::new(ModeRegistry::build_default());

    if !memory_only {
        // Replay sessions from logs
        let session_ids = storage.list_session_ids().await?;
        let mut recovered = 0usize;
        for session_id in session_ids {
            let log_entries = match storage.load_log(&session_id).await {
                Ok(entries) => entries,
                Err(e) if strict_recovery => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("strict recovery: failed to load log for {session_id}: {e}"),
                    )
                    .into());
                }
                Err(e) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %e,
                        "failed to load session log; skipping"
                    );
                    continue;
                }
            };
            if log_entries.is_empty() {
                continue;
            }

            match replay_session(&session_id, &log_entries, &mode_registry) {
                Ok(session) => {
                    if let Err(e) = storage.save_session(&session).await {
                        if strict_recovery {
                            return Err(io::Error::other(format!(
                                "strict recovery: failed to persist recovered session {session_id}: {e}"
                            ))
                            .into());
                        }
                        tracing::warn!(
                            session_id = %session_id,
                            error = %e,
                            "failed to persist recovered session"
                        );
                    }

                    log_store.create_session_log(&session_id).await;
                    for log_entry in &log_entries {
                        log_store.append(&session_id, log_entry.clone()).await;
                    }

                    registry.insert_recovered_session(session_id, session).await;
                    recovered += 1;
                }
                Err(e) if strict_recovery => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("strict recovery: failed to replay session {session_id}: {e}"),
                    )
                    .into());
                }
                Err(e) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %e,
                        "failed to replay session; skipping"
                    );
                }
            }
        }
        if recovered > 0 {
            tracing::info!(
                count = recovered,
                strict_recovery,
                "replayed sessions from log"
            );
        }
    }

    let runtime = Arc::new(Runtime::with_mode_registry(
        Arc::clone(&storage),
        Arc::clone(&registry),
        Arc::clone(&log_store),
        mode_registry,
    ));
    let security = SecurityLayer::from_env()?;
    let svc = MacpServer::new(Arc::clone(&runtime), security);

    let allow_insecure = std::env::var("MACP_ALLOW_INSECURE").ok().as_deref() == Some("1");
    let tls_cert = std::env::var("MACP_TLS_CERT_PATH").ok();
    let tls_key = std::env::var("MACP_TLS_KEY_PATH").ok();

    tracing::info!(%addr, "macp-runtime v0.4.0 listening");

    let builder = Server::builder();
    let mut builder = match (tls_cert, tls_key) {
        (Some(cert_path), Some(key_path)) => {
            let cert = tokio::fs::read(cert_path).await?;
            let key = tokio::fs::read(key_path).await?;
            builder.tls_config(ServerTlsConfig::new().identity(Identity::from_pem(cert, key)))?
        }
        _ if allow_insecure => {
            tracing::warn!(
                "starting without TLS because MACP_ALLOW_INSECURE=1; this is not RFC-compliant"
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
            tracing::info!("shutting down gracefully...");
        }
    }

    // Final snapshot: persist all sessions to storage
    if !memory_only {
        for session in registry.get_all_sessions().await {
            if let Err(e) = storage.save_session(&session).await {
                tracing::warn!(
                    session_id = %session.session_id,
                    error = %e,
                    "failed to persist session on shutdown"
                );
            }
        }
    }
    tracing::info!("state persisted, goodbye");

    Ok(())
}
