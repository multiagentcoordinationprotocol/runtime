use std::net::TcpListener;
use std::process::{Child, Command};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use macp_runtime::pb::InitializeRequest;

/// Manages the lifecycle of a local MACP runtime server subprocess.
pub struct ServerManager {
    process: Option<Child>,
    pub endpoint: String,
}

impl ServerManager {
    /// Start a runtime server on a free port. Returns once the server is accepting connections.
    pub async fn start(binary_path: &str) -> Result<Self> {
        let port = find_free_port()?;
        let bind_addr = format!("127.0.0.1:{port}");
        let endpoint = format!("http://{bind_addr}");

        tracing::info!("Starting MACP runtime: {binary_path} on {bind_addr}");

        let child = Command::new(binary_path)
            .env("MACP_ALLOW_INSECURE", "1")
            .env("MACP_MEMORY_ONLY", "1")
            .env("MACP_BIND_ADDR", &bind_addr)
            .env("RUST_LOG", "warn")
            .spawn()
            .with_context(|| format!("failed to start runtime binary: {binary_path}"))?;

        let mut manager = Self {
            process: Some(child),
            endpoint: endpoint.clone(),
        };

        if let Err(e) = wait_for_ready(&endpoint).await {
            manager.stop();
            bail!("Server failed to become ready: {e}");
        }

        tracing::info!("MACP runtime is ready at {endpoint}");
        Ok(manager)
    }

    /// Send SIGTERM and wait for the process to exit.
    pub fn stop(&mut self) {
        if let Some(mut child) = self.process.take() {
            tracing::info!("Stopping MACP runtime (pid={})", child.id());
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for ServerManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Find a free TCP port by binding to port 0 and reading the assigned port.
fn find_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Poll the server with Initialize RPCs until it responds or timeout.
async fn wait_for_ready(endpoint: &str) -> Result<()> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut interval = tokio::time::interval(Duration::from_millis(100));

    loop {
        interval.tick().await;

        if tokio::time::Instant::now() > deadline {
            bail!("Timed out waiting for server at {endpoint}");
        }

        match MacpRuntimeServiceClient::connect(endpoint.to_string()).await {
            Ok(mut client) => {
                let req = InitializeRequest {
                    supported_protocol_versions: vec!["1.0".into()],
                    client_info: None,
                    capabilities: None,
                };
                if client.initialize(req).await.is_ok() {
                    return Ok(());
                }
            }
            Err(_) => continue,
        }
    }
}
