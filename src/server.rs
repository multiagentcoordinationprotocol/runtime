use macp_runtime::error::MacpError;
use macp_runtime::mode::standard_mode_descriptors;
use macp_runtime::pb::macp_runtime_service_server::MacpRuntimeService;
use macp_runtime::pb::{
    Ack, CancelSessionRequest, CancelSessionResponse, CancellationCapability, Capabilities,
    Envelope, GetManifestRequest, GetManifestResponse, GetSessionRequest, GetSessionResponse,
    InitializeRequest, InitializeResponse, ListModesRequest, ListModesResponse, ListRootsRequest,
    ListRootsResponse, MacpError as PbMacpError, ManifestCapability, ModeRegistryCapability,
    ProgressCapability, RootsCapability, RuntimeInfo, SendRequest, SendResponse, SessionMetadata,
    SessionState as PbSessionState, SessionsCapability, StreamSessionRequest,
    StreamSessionResponse, WatchModeRegistryRequest, WatchModeRegistryResponse, WatchRootsRequest,
    WatchRootsResponse,
};
use macp_runtime::runtime::Runtime;
use macp_runtime::security::{AuthIdentity, SecurityLayer};
use macp_runtime::session::SessionState;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct MacpServer {
    runtime: Arc<Runtime>,
    security: SecurityLayer,
}

impl MacpServer {
    pub fn new(runtime: Arc<Runtime>, security: SecurityLayer) -> Self {
        Self { runtime, security }
    }

    fn validate_envelope_shape(&self, env: &Envelope) -> Result<(), MacpError> {
        if env.macp_version != "1.0" {
            return Err(MacpError::InvalidMacpVersion);
        }
        if env.message_type.is_empty() || env.message_id.is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        if env.message_type != "Signal" && env.session_id.is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        if env.message_type != "Signal" && env.mode.trim().is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        if env.payload.len() > self.security.max_payload_bytes {
            return Err(MacpError::PayloadTooLarge);
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

    fn apply_authenticated_sender(
        identity: &AuthIdentity,
        mut env: Envelope,
    ) -> Result<Envelope, MacpError> {
        if !env.sender.is_empty() && env.sender != identity.sender {
            return Err(MacpError::Unauthenticated);
        }
        env.sender = identity.sender.clone();
        Ok(env)
    }

    async fn authenticate_send_request(
        &self,
        request: &Request<SendRequest>,
        env: Envelope,
    ) -> Result<(Envelope, Option<usize>), MacpError> {
        let identity = self.security.authenticate_metadata(request.metadata())?;
        let env = Self::apply_authenticated_sender(&identity, env)?;
        let is_session_start = env.message_type == "SessionStart";
        self.security
            .authorize_mode(&identity, &env.mode, is_session_start)?;
        self.security
            .enforce_rate_limit(&identity.sender, is_session_start)
            .await?;
        // max_open_sessions is passed to runtime.process() where it is
        // enforced atomically under the session write lock, avoiding a
        // TOCTOU race between the count check and session insertion.
        let max_open = if is_session_start {
            identity.max_open_sessions
        } else {
            None
        };
        Ok((env, max_open))
    }

    async fn authenticate_session_access<T>(
        &self,
        request: &Request<T>,
        session_id: &str,
    ) -> Result<AuthIdentity, Status> {
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        let session = self
            .runtime
            .get_session_checked(session_id)
            .await
            .ok_or_else(|| Status::not_found(format!("Session '{}' not found", session_id)))?;
        let allowed = session.initiator_sender == identity.sender
            || session.participants.iter().any(|p| p == &identity.sender);
        if !allowed {
            return Err(Status::permission_denied(
                "FORBIDDEN: session access denied",
            ));
        }
        Ok(identity)
    }

    fn status_from_error(err: MacpError) -> Status {
        match err {
            MacpError::Unauthenticated => Status::unauthenticated(err.to_string()),
            MacpError::Forbidden => Status::permission_denied(err.to_string()),
            MacpError::PayloadTooLarge => Status::resource_exhausted(err.to_string()),
            MacpError::RateLimited => Status::resource_exhausted(err.to_string()),
            _ => Status::failed_precondition(err.to_string()),
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
        if !req.supported_protocol_versions.iter().any(|v| v == "1.0") {
            return Err(Status::failed_precondition(
                "UNSUPPORTED_PROTOCOL_VERSION: no mutually supported protocol version",
            ));
        }

        Ok(Response::new(InitializeResponse {
            selected_protocol_version: "1.0".into(),
            runtime_info: Some(RuntimeInfo {
                name: "macp-runtime".into(),
                title: "MACP Reference Runtime".into(),
                version: "0.4.0".into(),
                description: "Reference implementation of the Multi-Agent Coordination Protocol"
                    .into(),
                website_url: String::new(),
            }),
            capabilities: Some(Capabilities {
                sessions: Some(SessionsCapability { stream: false }),
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
            supported_modes: self.runtime.registered_mode_names(),
            instructions: "Authenticate requests with Authorization: Bearer <token>. For local development only, x-macp-agent-id may be enabled by configuration.".into(),
        }))
    }

    async fn send(&self, request: Request<SendRequest>) -> Result<Response<SendResponse>, Status> {
        let env = request
            .get_ref()
            .envelope
            .clone()
            .ok_or_else(|| Status::invalid_argument("SendRequest must contain an envelope"))?;

        let result = async {
            self.validate_envelope_shape(&env)?;
            let (env, max_open) = self.authenticate_send_request(&request, env).await?;
            self.runtime
                .process(&env, max_open)
                .await
                .map(|process_result| (env, process_result))
        }
        .await;

        let ack = match result {
            Ok((env, process_result)) => Ack {
                ok: true,
                duplicate: process_result.duplicate,
                message_id: env.message_id.clone(),
                session_id: env.session_id.clone(),
                accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                session_state: Self::session_state_to_pb(&process_result.session_state),
                error: None,
            },
            Err(err) => {
                let env = request.get_ref().envelope.clone().unwrap_or_default();
                Self::make_error_ack(&err, &env)
            }
        };

        Ok(Response::new(SendResponse { ack: Some(ack) }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let session_id = request.get_ref().session_id.clone();
        let _identity = self
            .authenticate_session_access(&request, &session_id)
            .await?;
        let session = self
            .runtime
            .get_session_checked(&session_id)
            .await
            .ok_or_else(|| Status::not_found(format!("Session '{}' not found", session_id)))?;

        Ok(Response::new(GetSessionResponse {
            metadata: Some(SessionMetadata {
                session_id: session.session_id.clone(),
                mode: session.mode.clone(),
                state: Self::session_state_to_pb(&session.state),
                started_at_unix_ms: session.started_at_unix_ms,
                expires_at_unix_ms: session.ttl_expiry,
                mode_version: session.mode_version.clone(),
                configuration_version: session.configuration_version.clone(),
                policy_version: session.policy_version.clone(),
            }),
        }))
    }

    async fn cancel_session(
        &self,
        request: Request<CancelSessionRequest>,
    ) -> Result<Response<CancelSessionResponse>, Status> {
        let session_id = request.get_ref().session_id.clone();
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        let session = self
            .runtime
            .get_session_checked(&session_id)
            .await
            .ok_or_else(|| Status::not_found(format!("Session '{}' not found", session_id)))?;
        if identity.sender != session.initiator_sender {
            return Err(Status::permission_denied(
                "FORBIDDEN: only the session initiator can cancel",
            ));
        }
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
            Err(err) => Ok(Response::new(CancelSessionResponse {
                ack: Some(Ack {
                    ok: false,
                    duplicate: false,
                    message_id: String::new(),
                    session_id: req.session_id.clone(),
                    accepted_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                    session_state: PbSessionState::Unspecified.into(),
                    error: Some(PbMacpError {
                        code: err.error_code().into(),
                        message: err.to_string(),
                        session_id: req.session_id,
                        message_id: String::new(),
                        details: vec![],
                    }),
                }),
            })),
        }
    }

    async fn get_manifest(
        &self,
        request: Request<GetManifestRequest>,
    ) -> Result<Response<GetManifestResponse>, Status> {
        let req = request.into_inner();
        if !req.agent_id.is_empty() && req.agent_id != "macp-runtime" {
            return Err(Status::not_found(format!(
                "Agent '{}' not found",
                req.agent_id
            )));
        }

        Ok(Response::new(GetManifestResponse {
            manifest: Some(macp_runtime::pb::AgentManifest {
                agent_id: "macp-runtime".into(),
                title: "MACP Reference Runtime".into(),
                description: "Reference implementation of MACP".into(),
                supported_modes: self.runtime.registered_mode_names(),
                input_content_types: vec!["application/macp-envelope+proto".into()],
                output_content_types: vec!["application/macp-envelope+proto".into()],
                metadata: HashMap::new(),
                transport_endpoints: vec![],
            }),
        }))
    }

    async fn list_modes(
        &self,
        _request: Request<ListModesRequest>,
    ) -> Result<Response<ListModesResponse>, Status> {
        Ok(Response::new(ListModesResponse {
            modes: standard_mode_descriptors(),
        }))
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
        _request: Request<tonic::Streaming<StreamSessionRequest>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        Err(Status::unimplemented(
            "StreamSession is intentionally disabled in the unary freeze profile",
        ))
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
    use prost::Message;

    fn make_server() -> (MacpServer, Arc<Runtime>) {
        let storage: Arc<dyn macp_runtime::storage::StorageBackend> =
            Arc::new(macp_runtime::storage::MemoryBackend);
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let runtime = Arc::new(Runtime::new(storage, registry, log_store));
        let server = MacpServer::new(runtime.clone(), SecurityLayer::dev_mode());
        (server, runtime)
    }

    fn send_req(sender: &str, env: Envelope) -> Request<SendRequest> {
        let mut req = Request::new(SendRequest {
            envelope: Some(env),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", sender.parse().unwrap());
        req
    }

    async fn do_send(server: &MacpServer, sender: &str, env: Envelope) -> Ack {
        let resp = server.send(send_req(sender, env)).await.unwrap();
        resp.into_inner().ack.unwrap()
    }

    fn start_payload() -> Vec<u8> {
        SessionStartPayload {
            intent: "intent".into(),
            participants: vec!["agent://fraud".into()],
            mode_version: "1.0.0".into(),
            configuration_version: "cfg-1".into(),
            policy_version: "policy-1".into(),
            ttl_ms: 1000,
            context: vec![],
            roots: vec![],
        }
        .encode_to_vec()
    }

    #[tokio::test]
    async fn sender_is_derived_from_authenticated_metadata() {
        let (server, runtime) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);
        let session = runtime.get_session_checked("s1").await.unwrap();
        assert_eq!(session.initiator_sender, "agent://orchestrator");
    }

    #[tokio::test]
    async fn spoofed_sender_is_rejected() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: "agent://spoof".into(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "UNAUTHENTICATED");
    }

    #[tokio::test]
    async fn get_session_requires_session_membership() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(GetSessionRequest {
            session_id: "s1".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://outsider".parse().unwrap());
        let err = server.get_session(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    // Note: stream_session cannot be unit-tested directly because
    // tonic::Streaming requires an HTTP/2 body. The endpoint returns
    // Status::unimplemented, verified via integration testing.

    #[tokio::test]
    async fn list_modes_returns_standard_modes() {
        let (server, _) = make_server();
        let resp = server
            .list_modes(Request::new(ListModesRequest {}))
            .await
            .unwrap();
        let names: Vec<String> = resp
            .into_inner()
            .modes
            .iter()
            .map(|m| m.mode.clone())
            .collect();
        assert_eq!(names.len(), 5);
        assert!(names.contains(&"macp.mode.decision.v1".to_string()));
        assert!(names.contains(&"macp.mode.proposal.v1".to_string()));
        assert!(names.contains(&"macp.mode.task.v1".to_string()));
        assert!(names.contains(&"macp.mode.handoff.v1".to_string()));
        assert!(names.contains(&"macp.mode.quorum.v1".to_string()));
    }

    #[tokio::test]
    async fn get_session_returns_metadata() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(GetSessionRequest {
            session_id: "s1".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://orchestrator".parse().unwrap());
        let resp = server.get_session(req).await.unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.session_id, "s1");
        assert_eq!(meta.mode, "macp.mode.decision.v1");
        assert_eq!(meta.mode_version, "1.0.0");
        assert_eq!(meta.configuration_version, "cfg-1");
    }

    #[tokio::test]
    async fn cancel_session_transitions_to_expired() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(CancelSessionRequest {
            session_id: "s1".into(),
            reason: "no longer needed".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://orchestrator".parse().unwrap());
        let resp = server.cancel_session(req).await.unwrap();
        let ack = resp.into_inner().ack.unwrap();
        assert!(ack.ok);
        assert_eq!(ack.session_state, PbSessionState::Expired as i32);
    }

    #[tokio::test]
    async fn participant_cannot_cancel_session() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: "s1".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        // Participant (not initiator) tries to cancel
        let mut req = Request::new(CancelSessionRequest {
            session_id: "s1".into(),
            reason: "I want to cancel".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://fraud".parse().unwrap());
        let err = server.cancel_session(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[tokio::test]
    async fn cancel_session_unknown_session_returns_error() {
        let (server, _) = make_server();
        // authenticate_session_access will fail with NotFound for unknown session
        let mut req = Request::new(CancelSessionRequest {
            session_id: "nonexistent".into(),
            reason: "test".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://orchestrator".parse().unwrap());
        let err = server.cancel_session(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }
}
