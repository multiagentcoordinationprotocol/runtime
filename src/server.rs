use macp_runtime::error::MacpError;
use macp_runtime::pb::macp_runtime_service_server::MacpRuntimeService;
use macp_runtime::pb::{
    Ack, CancelSessionRequest, CancelSessionResponse, CancellationCapability, Capabilities,
    Envelope, GetManifestRequest, GetManifestResponse, GetSessionRequest, GetSessionResponse,
    InitializeRequest, InitializeResponse, ListExtModesRequest, ListExtModesResponse,
    ListModesRequest, ListModesResponse, ListRootsRequest, ListRootsResponse,
    MacpError as PbMacpError, ManifestCapability, ModeRegistryCapability, ParticipantActivity,
    ProgressCapability, PromoteModeRequest, PromoteModeResponse, RegisterExtModeRequest,
    RegisterExtModeResponse, RootsCapability, RuntimeInfo, SendRequest, SendResponse,
    SessionMetadata, SessionState as PbSessionState, SessionsCapability, StreamSessionRequest,
    StreamSessionResponse, UnregisterExtModeRequest, UnregisterExtModeResponse,
    WatchModeRegistryRequest, WatchModeRegistryResponse, WatchRootsRequest, WatchRootsResponse,
    WatchSignalsRequest, WatchSignalsResponse,
};
use macp_runtime::runtime::Runtime;
use macp_runtime::security::{AuthIdentity, SecurityLayer};
use macp_runtime::session::SessionState;
use std::collections::HashMap;
use std::sync::Arc;
use tonic::{Request, Response, Status};

type SessionResponseStream = std::pin::Pin<
    Box<dyn futures_core::Stream<Item = Result<StreamSessionResponse, Status>> + Send>,
>;

#[derive(Clone)]
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
        if env.message_type == "Signal" {
            if !env.session_id.is_empty() {
                return Err(MacpError::InvalidEnvelope);
            }
            if !env.mode.trim().is_empty() {
                return Err(MacpError::InvalidEnvelope);
            }
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

    fn try_next_stream_event(
        receiver: &mut Option<tokio::sync::broadcast::Receiver<Envelope>>,
    ) -> Result<Option<Envelope>, Status> {
        use tokio::sync::broadcast::error::TryRecvError;

        let rx = match receiver.as_mut() {
            Some(rx) => rx,
            None => return Ok(None),
        };

        match rx.try_recv() {
            Ok(envelope) => Ok(Some(envelope)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Closed) => {
                *receiver = None;
                Ok(None)
            }
            Err(TryRecvError::Lagged(skipped)) => Err(Status::resource_exhausted(format!(
                "StreamSession receiver fell behind by {skipped} envelopes"
            ))),
        }
    }

    async fn process_stream_request(
        &self,
        identity: &AuthIdentity,
        req: StreamSessionRequest,
        bound_session_id: &mut Option<String>,
        session_events: &mut Option<tokio::sync::broadcast::Receiver<Envelope>>,
    ) -> Result<(), Status> {
        let envelope = req.envelope.ok_or_else(|| {
            Status::invalid_argument("StreamSessionRequest must contain an envelope")
        })?;

        self.validate_envelope_shape(&envelope)
            .map_err(Self::status_from_error)?;
        if envelope.session_id.trim().is_empty() {
            return Err(Status::invalid_argument(
                "StreamSession requires a non-empty session_id",
            ));
        }
        if envelope.mode.trim().is_empty() {
            return Err(Status::invalid_argument(
                "StreamSession requires a non-empty mode",
            ));
        }
        if let Some(bound) = bound_session_id.as_ref() {
            if bound != &envelope.session_id {
                return Err(Status::failed_precondition(
                    "StreamSession may only carry envelopes for one session_id",
                ));
            }
        }

        let envelope = Self::apply_authenticated_sender(identity, envelope)
            .map_err(Self::status_from_error)?;
        let is_session_start = envelope.message_type == "SessionStart";

        if !is_session_start {
            if let Some(session) = self.runtime.get_session_checked(&envelope.session_id).await {
                if envelope.mode != session.mode {
                    return Err(Status::failed_precondition(
                        "INVALID_ENVELOPE: envelope mode does not match the bound session mode",
                    ));
                }
                if session.state != SessionState::Open {
                    return Err(Status::failed_precondition("SESSION_NOT_OPEN"));
                }
            } else if envelope.message_type == "Signal" {
                return Err(Status::not_found(format!(
                    "Session '{}' not found",
                    envelope.session_id
                )));
            }
        }

        self.security
            .authorize_mode(identity, &envelope.mode, is_session_start)
            .map_err(Self::status_from_error)?;
        self.security
            .enforce_rate_limit(&identity.sender, is_session_start)
            .await
            .map_err(Self::status_from_error)?;

        if session_events.is_none() {
            *bound_session_id = Some(envelope.session_id.clone());
            *session_events = Some(self.runtime.subscribe_session_stream(&envelope.session_id));
        }

        let max_open = if is_session_start {
            identity.max_open_sessions
        } else {
            None
        };
        self.runtime
            .process(&envelope, max_open)
            .await
            .map_err(Self::status_from_error)?;
        Ok(())
    }

    fn build_stream_session_stream<S>(
        &self,
        identity: AuthIdentity,
        inbound: S,
    ) -> SessionResponseStream
    where
        S: futures_core::Stream<Item = Result<StreamSessionRequest, Status>> + Send + 'static,
    {
        use tokio::sync::broadcast;
        use tokio_stream::StreamExt;

        // Actions collected from tokio::select! arms to process outside the
        // select scope, avoiding borrow and macro-expansion issues with `?`
        // and `yield` inside select branches within try_stream!.
        enum StreamAction {
            ProcessRequest(StreamSessionRequest),
            EmitEnvelope(Envelope),
            ClientError(Status),
            ClientDone,
            EventsClosed,
            Lagged(u64),
        }

        let server = self.clone();
        let output = async_stream::try_stream! {
            let mut inbound = Box::pin(inbound);
            let mut bound_session_id: Option<String> = None;
            let mut session_events: Option<broadcast::Receiver<Envelope>> = None;

            loop {
                if session_events.is_some() {
                    let action = {
                        let events = session_events.as_mut().unwrap();
                        tokio::select! {
                            maybe_req = inbound.next() => {
                                match maybe_req {
                                    Some(Ok(req)) => StreamAction::ProcessRequest(req),
                                    Some(Err(status)) => StreamAction::ClientError(status),
                                    None => StreamAction::ClientDone,
                                }
                            }
                            recv_result = events.recv() => {
                                match recv_result {
                                    Ok(envelope) => StreamAction::EmitEnvelope(envelope),
                                    Err(broadcast::error::RecvError::Closed) => StreamAction::EventsClosed,
                                    Err(broadcast::error::RecvError::Lagged(n)) => StreamAction::Lagged(n),
                                }
                            }
                        }
                    };

                    match action {
                        StreamAction::ProcessRequest(req) => {
                            server
                                .process_stream_request(
                                    &identity,
                                    req,
                                    &mut bound_session_id,
                                    &mut session_events,
                                )
                                .await?;
                            while let Some(envelope) = Self::try_next_stream_event(&mut session_events)? {
                                yield StreamSessionResponse {
                                    envelope: Some(envelope),
                                };
                            }
                        }
                        StreamAction::EmitEnvelope(envelope) => {
                            yield StreamSessionResponse {
                                envelope: Some(envelope),
                            };
                        }
                        StreamAction::ClientError(status) => {
                            Err(status)?;
                        }
                        StreamAction::ClientDone => {
                            while let Some(envelope) = Self::try_next_stream_event(&mut session_events)? {
                                yield StreamSessionResponse {
                                    envelope: Some(envelope),
                                };
                            }
                            break;
                        }
                        StreamAction::EventsClosed => {
                            session_events = None;
                        }
                        StreamAction::Lagged(skipped) => {
                            Err(Status::resource_exhausted(format!(
                                "StreamSession receiver fell behind by {skipped} envelopes"
                            )))?;
                        }
                    }
                } else {
                    match inbound.next().await {
                        Some(Ok(req)) => {
                            server
                                .process_stream_request(
                                    &identity,
                                    req,
                                    &mut bound_session_id,
                                    &mut session_events,
                                )
                                .await?;
                            while let Some(envelope) = Self::try_next_stream_event(&mut session_events)? {
                                yield StreamSessionResponse {
                                    envelope: Some(envelope),
                                };
                            }
                        }
                        Some(Err(status)) => Err(status)?,
                        None => break,
                    }
                }
            }
        };
        Box::pin(output)
    }

    fn status_from_error(err: MacpError) -> Status {
        match err {
            MacpError::Unauthenticated => Status::unauthenticated(err.to_string()),
            MacpError::Forbidden => Status::permission_denied(err.to_string()),
            MacpError::PayloadTooLarge => Status::resource_exhausted(err.to_string()),
            MacpError::RateLimited => Status::resource_exhausted(err.to_string()),
            MacpError::StorageFailed => Status::internal(err.to_string()),
            MacpError::InvalidSessionId => Status::invalid_argument(err.to_string()),
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
                sessions: Some(SessionsCapability { stream: true }),
                cancellation: Some(CancellationCapability {
                    cancel_session: true,
                }),
                progress: Some(ProgressCapability { progress: true }),
                manifest: Some(ManifestCapability { get_manifest: true }),
                mode_registry: Some(ModeRegistryCapability {
                    list_modes: true,
                    list_changed: true,
                }),
                roots: Some(RootsCapability {
                    list_roots: true,
                    list_changed: true,
                }),
                experimental: Some(macp_runtime::pb::ExperimentalCapabilities {
                    features: HashMap::from([
                        ("ext_mode_lifecycle".into(), "true".into()),
                    ]),
                }),
            }),
            supported_modes: self.runtime.registered_mode_names(),
            instructions: "Authenticate requests with Authorization: Bearer <token>. Use the unary Send RPC for all session messaging. For local development only, x-macp-agent-id may be enabled by configuration.".into(),
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

        let participant_activity = session
            .participant_message_counts
            .iter()
            .map(|(pid, count)| ParticipantActivity {
                participant_id: pid.clone(),
                last_message_at_unix_ms: session
                    .participant_last_seen
                    .get(pid)
                    .copied()
                    .unwrap_or(0),
                message_count: *count,
            })
            .collect();

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
                participants: session.participants.clone(),
                participant_activity,
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
                // Empty: unary-first profile has no dedicated transport endpoints.
                transport_endpoints: vec![],
            }),
        }))
    }

    async fn list_modes(
        &self,
        _request: Request<ListModesRequest>,
    ) -> Result<Response<ListModesResponse>, Status> {
        Ok(Response::new(ListModesResponse {
            modes: self.runtime.standard_mode_descriptors(),
        }))
    }

    async fn list_roots(
        &self,
        _request: Request<ListRootsRequest>,
    ) -> Result<Response<ListRootsResponse>, Status> {
        Ok(Response::new(ListRootsResponse { roots: vec![] }))
    }

    type StreamSessionStream = SessionResponseStream;

    async fn stream_session(
        &self,
        request: Request<tonic::Streaming<StreamSessionRequest>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        let inbound = request.into_inner();
        Ok(Response::new(
            self.build_stream_session_stream(identity, inbound),
        ))
    }

    type WatchModeRegistryStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<WatchModeRegistryResponse, Status>> + Send>,
    >;

    async fn watch_mode_registry(
        &self,
        _request: Request<WatchModeRegistryRequest>,
    ) -> Result<Response<Self::WatchModeRegistryStream>, Status> {
        let mut rx = self.runtime.subscribe_mode_changes();
        let stream = async_stream::try_stream! {
            // Send initial state
            yield WatchModeRegistryResponse {
                change: Some(macp_runtime::pb::RegistryChanged {
                    registry: "modes".into(),
                    observed_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                }),
            };
            // Wait for changes from register/unregister/promote
            while rx.recv().await.is_ok() {
                yield WatchModeRegistryResponse {
                    change: Some(macp_runtime::pb::RegistryChanged {
                        registry: "modes".into(),
                        observed_at_unix_ms: chrono::Utc::now().timestamp_millis(),
                    }),
                };
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    type WatchRootsStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<WatchRootsResponse, Status>> + Send>,
    >;

    async fn watch_roots(
        &self,
        _request: Request<WatchRootsRequest>,
    ) -> Result<Response<Self::WatchRootsStream>, Status> {
        let initial = WatchRootsResponse {
            change: Some(macp_runtime::pb::RootsChanged {
                observed_at_unix_ms: chrono::Utc::now().timestamp_millis(),
            }),
        };
        let stream = async_stream::try_stream! {
            yield initial;
            // Roots are static — keep the stream open but idle.
            std::future::pending::<()>().await;
        };
        Ok(Response::new(Box::pin(stream)))
    }

    type WatchSignalsStream = std::pin::Pin<
        Box<dyn futures_core::Stream<Item = Result<WatchSignalsResponse, Status>> + Send>,
    >;

    async fn watch_signals(
        &self,
        _request: Request<WatchSignalsRequest>,
    ) -> Result<Response<Self::WatchSignalsStream>, Status> {
        let mut rx = self.runtime.subscribe_signals();
        let stream = async_stream::try_stream! {
            while let Ok(envelope) = rx.recv().await {
                yield WatchSignalsResponse {
                    envelope: Some(envelope),
                };
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    // Extension mode lifecycle RPCs

    async fn list_ext_modes(
        &self,
        _request: Request<ListExtModesRequest>,
    ) -> Result<Response<ListExtModesResponse>, Status> {
        Ok(Response::new(ListExtModesResponse {
            modes: self.runtime.extension_mode_descriptors(),
        }))
    }

    async fn register_ext_mode(
        &self,
        request: Request<RegisterExtModeRequest>,
    ) -> Result<Response<RegisterExtModeResponse>, Status> {
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        self.security
            .authorize_mode_registry(&identity)
            .map_err(Self::status_from_error)?;
        let req = request.into_inner();
        let descriptor = req
            .descriptor
            .ok_or_else(|| Status::invalid_argument("descriptor required"))?;
        match self.runtime.register_extension(descriptor) {
            Ok(()) => Ok(Response::new(RegisterExtModeResponse {
                ok: true,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(RegisterExtModeResponse {
                ok: false,
                error: e,
            })),
        }
    }

    async fn unregister_ext_mode(
        &self,
        request: Request<UnregisterExtModeRequest>,
    ) -> Result<Response<UnregisterExtModeResponse>, Status> {
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        self.security
            .authorize_mode_registry(&identity)
            .map_err(Self::status_from_error)?;
        let req = request.into_inner();
        match self.runtime.unregister_extension(&req.mode) {
            Ok(()) => Ok(Response::new(UnregisterExtModeResponse {
                ok: true,
                error: String::new(),
            })),
            Err(e) => Ok(Response::new(UnregisterExtModeResponse {
                ok: false,
                error: e,
            })),
        }
    }

    async fn promote_mode(
        &self,
        request: Request<PromoteModeRequest>,
    ) -> Result<Response<PromoteModeResponse>, Status> {
        let identity = self
            .security
            .authenticate_metadata(request.metadata())
            .map_err(Self::status_from_error)?;
        self.security
            .authorize_mode_registry(&identity)
            .map_err(Self::status_from_error)?;
        let req = request.into_inner();
        let new_name = if req.promoted_mode_name.is_empty() {
            None
        } else {
            Some(req.promoted_mode_name.as_str())
        };
        match self.runtime.promote_mode(&req.mode, new_name) {
            Ok(final_name) => Ok(Response::new(PromoteModeResponse {
                ok: true,
                error: String::new(),
                mode: final_name,
            })),
            Err(e) => Ok(Response::new(PromoteModeResponse {
                ok: false,
                error: e,
                mode: String::new(),
            })),
        }
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

    fn new_sid() -> String {
        uuid::Uuid::new_v4().as_hyphenated().to_string()
    }

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
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);
        let session = runtime.get_session_checked(&sid).await.unwrap();
        assert_eq!(session.initiator_sender, "agent://orchestrator");
    }

    #[tokio::test]
    async fn spoofed_sender_is_rejected() {
        let (server, _) = make_server();
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid,
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
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(GetSessionRequest { session_id: sid });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://outsider".parse().unwrap());
        let err = server.get_session(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::PermissionDenied);
    }

    #[tokio::test]
    async fn register_ext_mode_requires_authenticated_registry_permission() {
        let storage: Arc<dyn macp_runtime::storage::StorageBackend> =
            Arc::new(macp_runtime::storage::MemoryBackend);
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let runtime = Arc::new(Runtime::new(storage, registry, log_store));
        let security = SecurityLayer::from_env().unwrap_or_else(|_| SecurityLayer::dev_mode());
        let server = MacpServer::new(runtime, security);

        let req = Request::new(RegisterExtModeRequest {
            descriptor: Some(macp_runtime::pb::ModeDescriptor {
                mode: "ext.custom.v1".into(),
                mode_version: "1.0.0".into(),
                message_types: vec!["SessionStart".into(), "Commitment".into()],
                ..Default::default()
            }),
        });
        let err = server.register_ext_mode(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unauthenticated);
    }

    fn stream_identity(sender: &str) -> AuthIdentity {
        AuthIdentity {
            sender: sender.into(),
            allowed_modes: None,
            can_start_sessions: true,
            max_open_sessions: None,
            can_manage_mode_registry: false,
        }
    }

    #[tokio::test]
    async fn stream_session_emits_accepted_envelopes_only() {
        use tokio_stream::{iter, StreamExt};

        let (server, _) = make_server();
        let sid = new_sid();
        let requests = iter(vec![Ok(StreamSessionRequest {
            envelope: Some(Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            }),
        })]);

        let mut stream =
            server.build_stream_session_stream(stream_identity("agent://orchestrator"), requests);

        let response = stream.next().await.unwrap().unwrap();
        let envelope = response.envelope.unwrap();
        assert_eq!(envelope.message_type, "SessionStart");
        assert_eq!(envelope.message_id, "m1");
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_session_rejects_mixed_session_ids() {
        use tokio_stream::{iter, StreamExt};

        let (server, _) = make_server();
        let sid1 = new_sid();
        let sid2 = new_sid();
        let requests = iter(vec![
            Ok(StreamSessionRequest {
                envelope: Some(Envelope {
                    macp_version: "1.0".into(),
                    mode: "macp.mode.decision.v1".into(),
                    message_type: "SessionStart".into(),
                    message_id: "m1".into(),
                    session_id: sid1.clone(),
                    sender: String::new(),
                    timestamp_unix_ms: Utc::now().timestamp_millis(),
                    payload: start_payload(),
                }),
            }),
            Ok(StreamSessionRequest {
                envelope: Some(Envelope {
                    macp_version: "1.0".into(),
                    mode: "macp.mode.decision.v1".into(),
                    message_type: "SessionStart".into(),
                    message_id: "m2".into(),
                    session_id: sid2,
                    sender: String::new(),
                    timestamp_unix_ms: Utc::now().timestamp_millis(),
                    payload: start_payload(),
                }),
            }),
        ]);

        let mut stream =
            server.build_stream_session_stream(stream_identity("agent://orchestrator"), requests);

        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.envelope.unwrap().session_id, sid1);
        let err = stream.next().await.unwrap().unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    }

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
        // multi_round is now an extension, not in ListModes
        assert!(!names.contains(&"ext.multi_round.v1".to_string()));
    }

    #[tokio::test]
    async fn list_ext_modes_returns_extensions() {
        let (server, _) = make_server();
        let resp = server
            .list_ext_modes(Request::new(ListExtModesRequest {}))
            .await
            .unwrap();
        let names: Vec<String> = resp
            .into_inner()
            .modes
            .iter()
            .map(|m| m.mode.clone())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"ext.multi_round.v1".to_string()));
    }

    #[tokio::test]
    async fn get_manifest_includes_all_modes() {
        let (server, _) = make_server();
        let resp = server
            .get_manifest(Request::new(macp_runtime::pb::GetManifestRequest {
                agent_id: String::new(),
            }))
            .await
            .unwrap();
        let manifest = resp.into_inner().manifest.unwrap();
        assert_eq!(manifest.supported_modes.len(), 6);
        assert!(manifest
            .supported_modes
            .contains(&"ext.multi_round.v1".to_string()));
    }

    #[tokio::test]
    async fn get_session_returns_metadata() {
        let (server, _) = make_server();
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(GetSessionRequest {
            session_id: sid.clone(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://orchestrator".parse().unwrap());
        let resp = server.get_session(req).await.unwrap();
        let meta = resp.into_inner().metadata.unwrap();
        assert_eq!(meta.session_id, sid);
        assert_eq!(meta.mode, "macp.mode.decision.v1");
        assert_eq!(meta.mode_version, "1.0.0");
        assert_eq!(meta.configuration_version, "cfg-1");
    }

    #[tokio::test]
    async fn cancel_session_transitions_to_expired() {
        let (server, _) = make_server();
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(CancelSessionRequest {
            session_id: sid,
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
        let sid = new_sid();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "SessionStart".into(),
                message_id: "m1".into(),
                session_id: sid.clone(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: start_payload(),
            },
        )
        .await;
        assert!(ack.ok);

        let mut req = Request::new(CancelSessionRequest {
            session_id: sid,
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
        let mut req = Request::new(CancelSessionRequest {
            session_id: "nonexistent".into(),
            reason: "test".into(),
        });
        req.metadata_mut()
            .insert("x-macp-agent-id", "agent://orchestrator".parse().unwrap());
        let err = server.cancel_session(req).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn ambient_signal_accepted() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: String::new(),
                message_type: "Signal".into(),
                message_id: "sig-1".into(),
                session_id: String::new(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: vec![],
            },
        )
        .await;
        assert!(ack.ok);
    }

    #[tokio::test]
    async fn signal_with_session_id_rejected() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: String::new(),
                message_type: "Signal".into(),
                message_id: "sig-2".into(),
                session_id: "some-session".into(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: vec![],
            },
        )
        .await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "INVALID_ENVELOPE");
    }

    #[tokio::test]
    async fn signal_with_mode_rejected() {
        let (server, _) = make_server();
        let ack = do_send(
            &server,
            "agent://orchestrator",
            Envelope {
                macp_version: "1.0".into(),
                mode: "macp.mode.decision.v1".into(),
                message_type: "Signal".into(),
                message_id: "sig-3".into(),
                session_id: String::new(),
                sender: String::new(),
                timestamp_unix_ms: Utc::now().timestamp_millis(),
                payload: vec![],
            },
        )
        .await;
        assert!(!ack.ok);
        assert_eq!(ack.error.as_ref().unwrap().code, "INVALID_ENVELOPE");
    }

    #[tokio::test]
    async fn manifest_advertises_stream_enabled() {
        let (server, _) = make_server();
        let resp = server
            .initialize(Request::new(InitializeRequest {
                supported_protocol_versions: vec!["1.0".into()],
                client_info: None,
                capabilities: None,
            }))
            .await
            .unwrap();
        let caps = resp.into_inner().capabilities.unwrap();
        assert!(caps.sessions.unwrap().stream);
    }
}
