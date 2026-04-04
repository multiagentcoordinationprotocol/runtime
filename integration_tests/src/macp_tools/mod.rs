pub mod commit;
pub mod decision;
pub mod handoff;
pub mod proposal;
pub mod query;
pub mod quorum;
pub mod session_start;
pub mod task;

use std::sync::Arc;

use macp_runtime::pb::macp_runtime_service_client::MacpRuntimeServiceClient;
use tokio::sync::Mutex;
use tonic::transport::Channel;

/// Shared gRPC client handle used by all MACP tools.
pub type SharedClient = Arc<Mutex<MacpRuntimeServiceClient<Channel>>>;

/// Create a shared client for use with MACP tools.
pub async fn shared_client(endpoint: &str) -> SharedClient {
    let client = MacpRuntimeServiceClient::connect(endpoint.to_string())
        .await
        .expect("failed to connect");
    Arc::new(Mutex::new(client))
}
