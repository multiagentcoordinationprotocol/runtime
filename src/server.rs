use macp_runtime::error::MacpError;
use macp_runtime::pb::macp_service_server::MacpService;
use macp_runtime::pb::{
    Envelope, GetSessionRequest, GetSessionResponse, SendMessageRequest, SendMessageResponse,
};
use macp_runtime::runtime::Runtime;
use macp_runtime::session::SessionState;
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
        if env.macp_version != "v1" {
            return Err(MacpError::InvalidMacpVersion);
        }
        if env.session_id.is_empty() || env.message_id.is_empty() {
            return Err(MacpError::InvalidEnvelope);
        }
        Ok(())
    }
}

#[tonic::async_trait]
impl MacpService for MacpServer {
    async fn send_message(
        &self,
        request: Request<SendMessageRequest>,
    ) -> Result<Response<SendMessageResponse>, Status> {
        let req = request.into_inner();
        let env = req
            .envelope
            .ok_or_else(|| Status::invalid_argument("missing envelope"))?;

        let result = async {
            Self::validate(&env)?;
            self.runtime.process(&env).await
        }
        .await;

        let resp = match result {
            Ok(_) => SendMessageResponse {
                accepted: true,
                error: "".into(),
            },
            Err(e) => SendMessageResponse {
                accepted: false,
                error: e.to_string(),
            },
        };

        Ok(Response::new(resp))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let query = request.into_inner();

        match self.runtime.registry.get_session(&query.session_id).await {
            Some(session) => {
                let state_str = match session.state {
                    SessionState::Open => "Open",
                    SessionState::Resolved => "Resolved",
                    SessionState::Expired => "Expired",
                };

                Ok(Response::new(GetSessionResponse {
                    session_id: session.session_id.clone(),
                    mode: session.mode.clone(),
                    state: state_str.into(),
                    ttl_expiry: session.ttl_expiry,
                    resolution: session.resolution.clone().unwrap_or_default(),
                    mode_state: session.mode_state.clone(),
                    participants: session.participants.clone(),
                }))
            }
            None => Err(Status::not_found(format!(
                "Session '{}' not found",
                query.session_id
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use macp_runtime::log_store::LogStore;
    use macp_runtime::registry::SessionRegistry;
    use macp_runtime::session::Session;

    fn make_server() -> (MacpServer, Arc<Runtime>) {
        let registry = Arc::new(SessionRegistry::new());
        let log_store = Arc::new(LogStore::new());
        let runtime = Arc::new(Runtime::new(registry, log_store));
        let server = MacpServer::new(runtime.clone());
        (server, runtime)
    }

    fn wrap(env: Envelope) -> SendMessageRequest {
        SendMessageRequest {
            envelope: Some(env),
        }
    }

    #[tokio::test]
    async fn expired_session_transitions_to_expired() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        // Insert a session that expired 1 second ago
        let expired_ttl = Utc::now().timestamp_millis() - 1000;
        runtime
            .registry
            .insert_session_for_test(
                "s_expired".into(),
                Session {
                    session_id: "s_expired".into(),
                    state: SessionState::Open,
                    ttl_expiry: expired_ttl,
                    resolution: None,
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                },
            )
            .await;

        let env = Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_expired".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let resp = server
            .send_message(Request::new(wrap(env)))
            .await
            .unwrap();
        let ack = resp.into_inner();
        assert!(!ack.accepted);
        assert_eq!(ack.error, "TtlExpired");

        // Verify the session state was set to Expired
        let s = runtime.registry.get_session("s_expired").await.unwrap();
        assert_eq!(s.state, SessionState::Expired);
    }

    #[tokio::test]
    async fn non_expired_session_stays_open() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        // Insert a session that expires far in the future
        let future_ttl = Utc::now().timestamp_millis() + 60_000;
        runtime
            .registry
            .insert_session_for_test(
                "s_alive".into(),
                Session {
                    session_id: "s_alive".into(),
                    state: SessionState::Open,
                    ttl_expiry: future_ttl,
                    resolution: None,
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                },
            )
            .await;

        let env = Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_alive".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let resp = server
            .send_message(Request::new(wrap(env)))
            .await
            .unwrap();
        let ack = resp.into_inner();
        assert!(ack.accepted);

        let s = runtime.registry.get_session("s_alive").await.unwrap();
        assert_eq!(s.state, SessionState::Open);
    }

    #[tokio::test]
    async fn resolved_session_not_overwritten_to_expired() {
        let (_, runtime) = make_server();
        let server = MacpServer::new(runtime.clone());

        // Insert a resolved session with an expired TTL
        let expired_ttl = Utc::now().timestamp_millis() - 1000;
        runtime
            .registry
            .insert_session_for_test(
                "s_resolved".into(),
                Session {
                    session_id: "s_resolved".into(),
                    state: SessionState::Resolved,
                    ttl_expiry: expired_ttl,
                    resolution: Some(b"resolve".to_vec()),
                    mode: "decision".into(),
                    mode_state: vec![],
                    participants: vec![],
                },
            )
            .await;

        let env = Envelope {
            macp_version: "v1".into(),
            mode: "decision".into(),
            message_type: "Message".into(),
            message_id: "m1".into(),
            session_id: "s_resolved".into(),
            sender: "test".into(),
            timestamp_unix_ms: Utc::now().timestamp_millis(),
            payload: b"hello".to_vec(),
        };

        let resp = server
            .send_message(Request::new(wrap(env)))
            .await
            .unwrap();
        let ack = resp.into_inner();
        assert!(!ack.accepted);
        assert_eq!(ack.error, "SessionNotOpen");

        // State should still be Resolved, not overwritten to Expired
        let s = runtime.registry.get_session("s_resolved").await.unwrap();
        assert_eq!(s.state, SessionState::Resolved);
    }
}
