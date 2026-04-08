use std::sync::OnceLock;

use macp_integration_tests::config::TestConfig;
use macp_integration_tests::server_manager::ServerManager;
use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use tokio::sync::Mutex;
use tonic::transport::Channel;

/// Global server instance (started once, shared across all tests in a binary).
static SERVER: OnceLock<Mutex<Option<ServerManager>>> = OnceLock::new();
static ENDPOINT: OnceLock<String> = OnceLock::new();

/// Get the runtime endpoint, starting a server if necessary.
pub async fn endpoint() -> &'static str {
    // Fast path: already initialized
    if let Some(ep) = ENDPOINT.get() {
        return ep.as_str();
    }

    // Slow path: initialize
    let config = TestConfig::from_env();

    if let Some(ep) = config.endpoint {
        let _ = ENDPOINT.set(ep);
        let _ = SERVER.set(Mutex::new(None));
        return ENDPOINT.get().unwrap().as_str();
    }

    // Need to start a local server
    let binary = config.binary_path.unwrap_or_else(|| {
        // Default: look for binary relative to integration_tests/
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{manifest_dir}/../target/debug/macp-runtime")
    });

    let manager = ServerManager::start(&binary)
        .await
        .expect("failed to start local MACP runtime");

    let ep = manager.endpoint.clone();
    let _ = ENDPOINT.set(ep);
    let _ = SERVER.set(Mutex::new(Some(manager)));

    ENDPOINT.get().unwrap().as_str()
}

/// Create a new gRPC client connected to the test runtime.
#[allow(dead_code)]
pub async fn grpc_client() -> MacpRuntimeServiceClient<Channel> {
    let ep = endpoint().await;
    MacpRuntimeServiceClient::connect(ep.to_string())
        .await
        .expect("failed to connect to runtime")
}
