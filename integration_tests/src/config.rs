use std::env;

/// Configuration for integration test target.
///
/// Supports three modes:
/// - **Local dev**: no env vars — builds parent crate, starts server on free port
/// - **CI**: `MACP_TEST_BINARY` set — uses pre-built binary, starts server
/// - **Hosted**: `MACP_TEST_ENDPOINT` set — connects directly, no server management
pub struct TestConfig {
    /// gRPC endpoint to connect to (e.g. "http://127.0.0.1:50051")
    pub endpoint: Option<String>,
    /// Use TLS for the connection
    pub use_tls: bool,
    /// Bearer token for hosted runtime authentication
    pub auth_token: Option<String>,
    /// Path to a pre-built runtime binary
    pub binary_path: Option<String>,
}

impl TestConfig {
    pub fn from_env() -> Self {
        Self {
            endpoint: env::var("MACP_TEST_ENDPOINT").ok(),
            use_tls: env::var("MACP_TEST_TLS").ok().as_deref() == Some("1"),
            auth_token: env::var("MACP_TEST_AUTH_TOKEN").ok(),
            binary_path: env::var("MACP_TEST_BINARY").ok(),
        }
    }

    /// Whether we need to start a local server (no external endpoint provided).
    pub fn needs_local_server(&self) -> bool {
        self.endpoint.is_none()
    }

    /// Whether to use dev-mode auth (Authorization: Bearer <sender>) instead of configured bearer tokens.
    pub fn use_dev_headers(&self) -> bool {
        self.auth_token.is_none()
    }
}
