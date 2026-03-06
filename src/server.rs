use macp_runtime::error::MacpError;
use macp_runtime::pb::macp_runtime_service_server::MacpRuntimeService;
use macp_runtime::pb::{
    Ack, CancelSessionRequest, CancelSessionResponse, CancellationCapability, Capabilities,
    Envelope, GetManifestRequest, GetManifestResponse, GetSessionRequest, GetSessionResponse,
    InitializeRequest, InitializeResponse, ListModesRequest, ListModesResponse, ListRootsRequest,
    ListRootsResponse, MacpError as PbMacpError, ManifestCapability, ModeDescriptor,
    ModeRegistryCapability, ProgressCapability, RootsCapability, RuntimeInfo, SendRequest,
    SendResponse, SessionMetadata, SessionState as PbSessionState, SessionsCapability,
    StreamSessionRequest, StreamSessionResponse, WatchModeRegistryRequest,
    WatchModeRegistryResponse, WatchRootsRequest, WatchRootsResponse,
};
use macp_runtime::runtime::Runtime;
use macp_runtime::session::SessionState;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct MacpServer {
    runtime: Arc<Runtime>,
}

impl MacpServer {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }

    fn validate(env: &Envelope) -> Result<(), MacpError> {
        if env.macp_version != "1.0" {
            return Err(MacpError::InvalidMacpVersion);
        }
        // Signals may have empty session_id
        if env.message_type != "Signal" && env.session_id.is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        if env.message_id.is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        Ok(())
    }

    fn session_state_to_pb(state: &SessionState) -> i32 {
        match state {
            SessionState::Open => PbSessionState::Open.into(),
            SessionState::Resolved => PbSessionState::Resolved.into(),
            SessionState::Expired => PbSessionState::Expired.into(),
        }
    }

    fn make_error_ack(e: &MacpError, env: &Envelope) -> Ack {
        Ack {
            ok: false,
            duplicate: false,
            message_id: env.message_id.clone(),
            session_id: env.session_id.clone(),
            accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
            session_state: PbSessionState::Unspecified.into(),
            error: Some(PbMacpError {
                code: e.error_code().into(),
                message: e.to_string(),
                session_id: env.session_id.clone(),
                message_id: env.message_id.clone(),
                details: vec![],
            }),
        }
    }
}

#[tonic::async_trait]
impl MacpRuntimeService for MacpServer {
    async fn initialize(
        &self,
        request: Request<InitializeRequest>,
    ) -> Result<Response<InitializeResponse>, Status> {
        let req = request.into_inner();

        // Negotiate protocol version
        let supported = &req.supported_protocol_versions;
        if !supported.iter().any(|v| v == "1.0") {
            return Err(Status::failed_precondition(
                "UNSUPPORTED_PROTOCOL_VERSION: no mutually supported protocol version",
            ));
        }

        let mode_names = self.runtime.registered_mode_names();

        Ok(Response::new(InitializeResponse {
            selected_protocol_version: "1.0".into(),
            runtime_info: Some(RuntimeInfo {
                name: "macp-runtime".into(),
                title: "MACP Reference Runtime".into(),
                version: "0.2".into(),
                description: "Reference implementation of the Multi-Agent Coordination Protocol"
                    .into(),
                website_url: String::new(),
            }),
            capabilities: Some(Capabilities {
                sessions: Some(SessionsCapability { stream: true }),
                cancellation: Some(CancellationCapability {
                    cancel_session: true,
                }),
                progress: Some(ProgressCapability { progress: false }),
                manifest: Some(ManifestCapability { get_manifest: true }),
                mode_registry: Some(ModeRegistryCapability {
                    list_modes: true,
                    list_changed: false,
                }),
                roots: Some(RootsCapability {
                    list_roots: true,
                    list_changed: false,
                }),
                experimental: None,
            }),
            supported_modes: mode_names,
            instructions: String::new(),
        }))
    }

    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let send_req = request.into_inner();
        let env = send_req
            .envelope
            .ok_or_else(|| Status::invalid_argument("SendRequest must contain an envelope"))?;

        let result = async {
            Self::validate(&env)?;
            self.runtime.process(&env).await
        }
        .await;

        let ack = match result {
            Ok(process_result) => Ack {
                ok: true,
                duplicate: process_result.duplicate,
                message_id: env.message_id.clone(),
                session_id: env.session_id.clone(),
                accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                session_state: Self::session_state_to_pb(&process_result.session_state),
                error: None,
            },
            Err(e) => Self::make_error_ack(&e, &env),
        };

        Ok(Response::new(SendResponse { ack: Some(ack) }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let req = request.into_inner();

        match self.runtime.registry.get_session(&req.session_id).await {
            Some(session) => {
                let state = Self::session_state_to_pb(&session.state);

                Ok(Response::new(GetSessionResponse {
                    metadata: Some(SessionMetadata {
                        session_id: session.session_id.clone(),
                        mode: session.mode.clone(),
                        state,
                        started_at_unix_ms: session.started_at_unix_ms,
                        expires_at_unix_ms: session.ttl_expiry,
                        mode_version: session.mode_version.clone(),
                        configuration_version: session.configuration_version.clone(),
                        policy_version: session.policy_version.clone(),
                    }),
                }))
            }
            None => Err(Status::not_found(format!(
                "Session '{}' not found",
                req.session_id
            ))),
        }
    }

    async fn cancel_session(
        &self,
        request: Request<CancelSessionRequest>,
    ) -> Result<Response<CancelSessionResponse>, Status> {
        let req = request.into_inner();

        match self
            .runtime
            .cancel_session(&req.session_id, &req.reason)
            .await
        {
            Ok(result) => Ok(Response::new(CancelSessionResponse {
                ack: Some(Ack {
                    ok: true,
                    duplicate: false,
                    message_id: String::new(),
                    session_id: req.session_id,
                    accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                    session_state: Self::session_state_to_pb(&result.session_state),
                    error: None,
                }),
            })),
            Err(e) => {
                let ack = Ack {
                    ok: false,
                    duplicate: false,
                    message_id: String::new(),
                    session_id: req.session_id.clone(),
                    accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                    session_state: PbSessionState::Unspecified.into(),
                    error: Some(PbMacpError {
                        code: e.error_code().into(),
                        message: e.to_string(),
                        session_id: req.session_id,
                        message_id: String::new(),
                        details: vec![],
                    }),
                };
                Ok(Response::new(CancelSessionResponse { ack: Some(ack) }))
            }
        }
    }

    async fn get_manifest(
        &self,
        _request: Request<GetManifestRequest>,
    ) -> Result<Response<GetManifestResponse>, Status> {
        let mode_names = self.runtime.registered_mode_names();

        Ok(Response::new(GetManifestResponse {
            manifest: Some(macp_runtime::pb::AgentManifest {
                agent_id: "macp-runtime".into(),
                title: "MACP Reference Runtime".into(),
                description: "Reference implementation of MACP".into(),
                supported_modes: mode_names,
                input_content_types: vec!["application/protobuf".into()],
                output_content_types: vec!["application/protobuf".into()],
                metadata: HashMap::new(),
            }),
        }))
    }

    async fn list_modes(
        &self,
        _request: Request<ListModesRequest>,
    ) -> Result<Response<ListModesResponse>, Status> {
        let modes = vec![
            ModeDescriptor {
                mode: "macp.mode.decision.v1".into(),
                mode_version: "1.0".into(),
                title: "Decision Mode".into(),
                description: "Proposal-based decision making with voting".into(),
                determinism_class: "semantic-deterministic".into(),
                participant_model: "open".into(),
                message_types: vec![
                    "Proposal".into(),
                    "Evaluation".into(),
                    "Objection".into(),
                    "Vote".into(),
                    "Commitment".into(),
                ],
                terminal_message_types: vec!["Commitment".into()],
                schema_uris: HashMap::new(),
            },
            ModeDescriptor {
                mode: "macp.mode.multi_round.v1".into(),
                mode_version: "1.0".into(),
                title: "Multi-Round Convergence Mode".into(),
                description: "Participant-based convergence with all_equal strategy".into(),
                determinism_class: "semantic-deterministic".into(),
                participant_model: "closed".into(),
                message_types: vec!["Contribute".into()],
                terminal_message_types: vec![],
                schema_uris: HashMap::new(),
            },
        ];

        Ok(Response::new(ListModesResponse { modes }))
    }

    async fn list_roots(
        &self,
        _request: Request<ListRootsRequest>,
    ) -> Result<Response<ListRootsResponse>, Status> {
        Ok(Response::new(ListRootsResponse { roots: vec![] }))
    }

    type StreamSessionStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<StreamSessionResponse, Status>> + Send>,
    >;

    async fn stream_session(
        &self,
        request: Request<tonic::Streaming<StreamSessionRequest>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        use tokio_stream::StreamExt;

        let runtime = self.runtime.clone();
        let mut inbound = request.into_inner();

        let output = async_stream::try_stream! {
            while let Some(req) = inbound.next().await {
                let req = req?;
                let env = req.envelope.ok_or_else(|| {
                    Status::invalid_argument("StreamSessionRequest must contain an envelope")
                })?;

                let result = async {
                    MacpServer::validate(&env)?;
                    runtime.process(&env).await
                }
                .await;

                match result {
                    Ok(_) => {
                        yield StreamSessionResponse {
                            envelope: Some(env),
                        };
                    }
                    Err(e) => {
                        // Build an error envelope as a response
                        let error_ack = MacpServer::make_error_ack(&e, &env);
                        let error_env = Envelope {
                            macp_version: "1.0".into(),
                            mode: env.mode.clone(),
                            message_type: "Error".into(),
                            message_id: format!("err-{}", env.message_id),
                            session_id: env.session_id.clone(),
                            sender: "_runtime".into(),
                            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                            payload: serde_json::to_vec(&serde_json::json!({
                                "code": error_ack.error.as_ref().map(|e| &e.code),
                                "message": error_ack.error.as_ref().map(|e| &e.message),
                            })).unwrap_or_default(),
                        };
                        yield StreamSessionResponse {
                            envelope: Some(error_env),
                        };
                    }
                }
            }
        };

        Ok(Response::new(Box::pin(output)))
    }

    type WatchModeRegistryStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<WatchModeRegistryResponse, Status>> + Send>,
    >;

    async fn watch_mode_registry(
        &self,
        _request: Request<WatchModeRegistryRequest>,
    ) -> Result<Response<Self::WatchModeRegistryStream>, Status> {
        Err(Status::unimplemented(
            "WatchModeRegistry is not yet implemented",
        ))
    }

    type WatchRootsStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<WatchRootsResponse, Status>> + Send>,
    >;

    async fn watch_roots(
        &self,
        _request: Request<WatchRootsRequest>,
    ) -> Result<Response<Self::WatchRootsStream>, Status> {
        Err(Status::unimplemented("WatchRoots is not yet implemented"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use macp_runtime::log_store::LogStore;
    use macp_runtime::pb::SessionStartPayload;
    use macp_runtime::registry::SessionRegistry;
    use macp_runtime::session::Session;
    use prost::Message;
    use std::collections::HashSet;

    fn make_server() -> (MacpServer, Arc<Runtime>) {
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let runtime = Arc::new(Runtime::new(registry, log_store));
        let server = MacpServer::new(runtime.clone());
        (server, runtime)
    }

    fn send_req(env: Envelope) -> Request<SendRequest> {
        Request::new(SendRequest {
            envelope: Some(env),
        })
    }

    async fn do_send(server: &MacpServer, env: Envelope) -> Ack {
        let resp = server.send(send_req(env)).await.unwrap();
        resp.into_inner().ack.unwrap()
    }

    // --- TTL/Session state tests ---

    #[tokio::test]
    async fn expired_session_transitions_to_expired() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        let expired_ttl = Utc::now().timestamp_millis() - 1000;
        runtime
            .registry
            .insert_session_for_test(
                "s_expired".into(),
                Session {
                    session_id: "s_expired".into(),
                    state: SessionState::Open,
                    ttl_expiry: expired_ttl,
                    started_at_unix_ms: expired_ttl - 60_000,
                    resolution: None,
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                    seen_message_ids: HashSet::new(),
                    intent: String::new(),
                    mode_version: String::new(),
                    configuration_version: String::new(),
                    policy_version: String::new(),
                },
            )
            .await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_expired".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "SESSION_NOT_OPEN");

        let s = runtime.registry.get_session("s_expired").await.unwrap();
        assert_eq!(s.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn non_expired_session_stays_open() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        let future_ttl = Utc::now().timestamp_millis() + 60_000;
        runtime
            .registry
            .insert_session_for_test(
                "s_alive".into(),
                Session {
                    session_id: "s_alive".into(),
                    state: SessionState::Open,
                    ttl_expiry: future_ttl,
                    started_at_unix_ms: future_ttl - 120_000,
                    resolution: None,
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                    seen_message_ids: HashSet::new(),
                    intent: String::new(),
                    mode_version: String::new(),
                    configuration_version: String::new(),
                    policy_version: String::new(),
                },
            )
            .await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_alive".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let ack = do_send(&server, env).await;
        assert!(ack.ok);

        let s = runtime.registry.get_session("s_alive").await.unwrap();
        assert_eq!(s.state, SessionState::Open);
    }

    #[tokio::test]
    async fn resolved_session_not_overwritten_to_expired() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        let expired_ttl = Utc::now().timestamp_millis() - 1000;
        runtime
            .registry
            .insert_session_for_test(
                "s_resolved".into(),
                Session {
                    session_id: "s_resolved".into(),
                    state: SessionState::Resolved,
                    ttl_expiry: expired_ttl,
                    started_at_unix_ms: expired_ttl - 60_000,
                    resolution: Some(b"resolve".to_vec()),
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                    seen_message_ids: HashSet::new(),
                    intent: String::new(),
                    mode_version: String::new(),
                    configuration_version: String::new(),
                    policy_version: String::new(),
                },
            )
            .await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_resolved".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "SESSION_NOT_OPEN");

        let s = runtime.registry.get_session("s_resolved").await.unwrap();
        assert_eq!(s.state, SessionState::Resolved);
    }

    // --- Version validation ---

    #[tokio::test]
    async fn version_1_0_accepted() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
    }

    #[tokio::test]
    async fn version_v1_rejected() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(
            ack.error.as_ref().unwrap().code,
            "UNSUPPORTED_PROTOCOL_VERSION"
        );
    }

    #[tokio::test]
    async fn empty_version_rejected() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: String::new(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(
            ack.error.as_ref().unwrap().code,
            "UNSUPPORTED_PROTOCOL_VERSION"
        );
    }

    // --- Envelope validation ---

    #[tokio::test]
    async fn empty_session_id_rejected() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: String::new(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "INVALID_ENVELOPE");
    }

    #[tokio::test]
    async fn empty_message_id_rejected() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: String::new(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "INVALID_ENVELOPE");
    }

    // --- Signal with empty session_id allowed ---

    #[tokio::test]
    async fn signal_empty_session_id_allowed() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: String::new(),
            message_type: "Signal".into(),
            message_id: "sig1".into(),
            session_id: String::new(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
    }

    // --- Ack fields populated correctly ---

    #[tokio::test]
    async fn ack_fields_on_success() {
        let (server, _) = make_server();
        let before = Utc::now().timestamp_millis();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "msg123".into(),
            session_id: "sess456".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;

        assert!(ack.ok);
        assert!(!ack.duplicate);
        assert_eq!(ack.message_id, "msg123");
        assert_eq!(ack.session_id, "sess456");
        assert!(ack.accepted_at_unix_ms >= before);
        assert_eq!(ack.session_state, PbSessionState::Open as i32);
        assert!(ack.error.is_none());
    }

    #[tokio::test]
    async fn ack_fields_on_error() {
        let (server, _) = make_server();
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "msg789".into(),
            session_id: "nonexistent".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;

        assert!(!ack.ok);
        assert!(!ack.duplicate);
        assert_eq!(ack.message_id, "msg789");
        assert_eq!(ack.session_id, "nonexistent");
        let err = ack.error.unwrap();
        assert_eq!(err.code, "SESSION_NOT_FOUND");
        assert_eq!(err.message, "UnknownSession");
        assert_eq!(err.session_id, "nonexistent");
        assert_eq!(err.message_id, "msg789");
    }

    // --- Deduplication at server layer ---

    #[tokio::test]
    async fn ack_duplicate_field_set() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert!(!ack.duplicate);

        // Send a message
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.duplicate);

        // Duplicate
        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert!(ack.duplicate);
    }

    // --- GetSession ---

    #[tokio::test]
    async fn get_session_success() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_get".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        do_send(&server, env).await;

        let resp = server
            .get_session(Request::new(GetSessionRequest {
                session_id: "s_get".into(),
            }))
            .await
            .unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.session_id, "s_get");
        assert_eq!(meta.mode, "decision");
        assert_eq!(meta.state, PbSessionState::Open as i32);
        assert!(meta.started_at_unix_ms > 0);
        assert!(meta.expires_at_unix_ms > meta.started_at_unix_ms);
    }

    #[tokio::test]
    async fn get_session_not_found() {
        let (server, _) = make_server();

        let result = server
            .get_session(Request::new(GetSessionRequest {
                session_id: "nonexistent".into(),
            }))
            .await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn get_session_shows_resolved_state() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_res".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        do_send(&server, env).await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s_res".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"resolve".to_vec(),
        };
        do_send(&server, env).await;

        let resp = server
            .get_session(Request::new(GetSessionRequest {
                session_id: "s_res".into(),
            }))
            .await
            .unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.state, PbSessionState::Resolved as i32);
    }

    #[tokio::test]
    async fn get_session_with_version_fields() {
        let (server, _) = make_server();

        let start_payload = SessionStartPayload {
            intent: "coordinate".into(),
            participants: vec![],
            mode_version: "1.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "pol-1".into(),
            ttl_ms: 0,
            context: vec![],
            roots: vec![],
        };

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_ver".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        };
        do_send(&server, env).await;

        let resp = server
            .get_session(Request::new(GetSessionRequest {
                session_id: "s_ver".into(),
            }))
            .await
            .unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.mode_version, "1.0");
        assert_eq!(meta.configuration_version, "cfg-1");
        assert_eq!(meta.policy_version, "pol-1");
    }

    // --- CancelSession at server layer ---

    #[tokio::test]
    async fn cancel_session_success() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_cancel".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        do_send(&server, env).await;

        let resp = server
            .cancel_session(Request::new(CancelSessionRequest {
                session_id: "s_cancel".into(),
                reason: "testing".into(),
            }))
            .await
            .unwrap();
        let ack = resp.into_inner().ack.unwrap();
        assert!(ack.ok);
        assert_eq!(ack.session_id, "s_cancel");
        assert_eq!(ack.session_state, PbSessionState::Expired as i32);
    }

    #[tokio::test]
    async fn cancel_session_not_found() {
        let (server, _) = make_server();

        let resp = server
            .cancel_session(Request::new(CancelSessionRequest {
                session_id: "nonexistent".into(),
                reason: "testing".into(),
            }))
            .await
            .unwrap();
        let ack = resp.into_inner().ack.unwrap();
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "SESSION_NOT_FOUND");
    }

    #[tokio::test]
    async fn cancel_then_message_rejected() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_cm".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        do_send(&server, env).await;

        server
            .cancel_session(Request::new(CancelSessionRequest {
                session_id: "s_cm".into(),
                reason: "done".into(),
            }))
            .await
            .unwrap();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s_cm".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "SESSION_NOT_OPEN");
    }

    // --- Participant validation at server layer ---

    #[tokio::test]
    async fn forbidden_error_at_server_layer() {
        let (server, _) = make_server();

        let start_payload = SessionStartPayload {
            intent: String::new(),
            participants: vec!["alice".into()],
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            ttl_ms: 0,
            context: vec![],
            roots: vec![],
        };

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_auth".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        };
        do_send(&server, env).await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s_auth".into(),
            sender: "bob".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "FORBIDDEN");
    }

    // --- Full decision flow through server ---

    #[tokio::test]
    async fn full_decision_flow_through_server() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s_flow".into(),
            sender: "alice".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Open as i32);

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m2".into(),
            session_id: "s_flow".into(),
            sender: "alice".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Open as i32);

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m3".into(),
            session_id: "s_flow".into(),
            sender: "alice".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"resolve".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Resolved as i32);

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m4".into(),
            session_id: "s_flow".into(),
            sender: "alice".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"too late".to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.unwrap().code, "SESSION_NOT_OPEN");
    }

    // --- Full multi-round flow through server ---

    #[tokio::test]
    async fn full_multi_round_flow_through_server() {
        let (server, _) = make_server();

        let start_payload = SessionStartPayload {
            intent: String::new(),
            ttl_ms: 60_000,
            participants: vec!["alice".into(), "bob".into()],
            mode_version: String::new(),
            configuration_version: String::new(),
            policy_version: String::new(),
            context: vec![],
            roots: vec![],
        };

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "SessionStart".into(),
            message_id: "m0".into(),
            session_id: "s_mr".into(),
            sender: "coordinator".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: start_payload.encode_to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Open as i32);

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m1".into(),
            session_id: "s_mr".into(),
            sender: "alice".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_a"}"#.to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Open as i32);

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "multi_round".into(),
            message_type: "Contribute".into(),
            message_id: "m2".into(),
            session_id: "s_mr".into(),
            sender: "bob".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: br#"{"value":"option_a"}"#.to_vec(),
        };
        let ack = do_send(&server, env).await;
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Resolved as i32);

        let resp = server
            .get_session(Request::new(GetSessionRequest {
                session_id: "s_mr".into(),
            }))
            .await
            .unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.state, PbSessionState::Resolved as i32);
    }

    // --- Unknown mode error code ---

    #[tokio::test]
    async fn unknown_mode_error_code() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "nonexistent_mode".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "MODE_NOT_SUPPORTED");
    }

    // --- Duplicate session error code ---

    #[tokio::test]
    async fn duplicate_session_error_code() {
        let (server, _) = make_server();

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m1".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        do_send(&server, env).await;

        let env = Envelope {
            macp_version: "1.0".into(),
            mode: "decision".into(),
            message_type: "SessionStart".into(),
            message_id: "m2".into(),
            session_id: "s1".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: vec![],
        };
        let ack = do_send(&server, env).await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "INVALID_ENVELOPE");
    }

    // --- Initialize RPC ---

    #[tokio::test]
    async fn initialize_success() {
        let (server, _) = make_server();

        let resp = server
            .initialize(Request::new(InitializeRequest {
                supported_protocol_versions: vec!["1.0".into()],
                client_info: None,
                capabilities: None,
            }))
            .await
            .unwrap();
        let init = resp.into_inner();
        assert_eq!(init.selected_protocol_version, "1.0");
        assert!(init.runtime_info.is_some());
        let info = init.runtime_info.unwrap();
        assert_eq!(info.name, "macp-runtime");
        assert_eq!(info.version, "0.2");
        assert!(init.capabilities.is_some());
        assert!(!init.supported_modes.is_empty());
    }

    #[tokio::test]
    async fn initialize_no_common_version() {
        let (server, _) = make_server();

        let result = server
            .initialize(Request::new(InitializeRequest {
                supported_protocol_versions: vec!["2.0".into()],
                client_info: None,
                capabilities: None,
            }))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn initialize_selects_1_0_from_multiple() {
        let (server, _) = make_server();

        let resp = server
            .initialize(Request::new(InitializeRequest {
                supported_protocol_versions: vec!["1.0".into(), "2.0".into()],
                client_info: None,
                capabilities: None,
            }))
            .await
            .unwrap();
        assert_eq!(resp.into_inner().selected_protocol_version, "1.0");
    }

    // --- ListModes ---

    #[tokio::test]
    async fn list_modes_returns_descriptors() {
        let (server, _) = make_server();

        let resp = server
            .list_modes(Request::new(ListModesRequest {}))
            .await
            .unwrap();
        let modes = resp.into_inner().modes;
        assert_eq!(modes.len(), 2);
        assert_eq!(modes[0].mode, "macp.mode.decision.v1");
        assert_eq!(modes[1].mode, "macp.mode.multi_round.v1");
    }

    // --- GetManifest ---

    #[tokio::test]
    async fn get_manifest_returns_runtime_manifest() {
        let (server, _) = make_server();

        let resp = server
            .get_manifest(Request::new(GetManifestRequest {
                agent_id: String::new(),
            }))
            .await
            .unwrap();
        let manifest = resp.into_inner().manifest.unwrap();
        assert_eq!(manifest.agent_id, "macp-runtime");
        assert!(!manifest.supported_modes.is_empty());
    }

    // --- ListRoots ---

    #[tokio::test]
    async fn list_roots_returns_empty() {
        let (server, _) = make_server();

        let resp = server
            .list_roots(Request::new(ListRootsRequest {}))
            .await
            .unwrap();
        assert!(resp.into_inner().roots.is_empty());
    }
}
