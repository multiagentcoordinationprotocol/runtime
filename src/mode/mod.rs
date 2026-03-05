pub mod decision;
pub mod multi_round;

use crate::error::MacpError;
use crate::pb::Envelope;
use crate::session::Session;

/// The result of a Mode processing a message.
/// The runtime applies this response to mutate session state.
#[derive(Debug)]
pub enum ModeResponse {
    /// No state change needed.
    NoOp,
    /// Persist updated mode state.
    PersistState(Vec<u8>),
    /// Resolve the session with the given resolution data.
    Resolve(Vec<u8>),
    /// Persist mode state and resolve in one step.
    PersistAndResolve { state: Vec<u8>, resolution: Vec<u8> },
}

/// Trait that coordination modes implement.
/// Modes receive immutable session references and return a ModeResponse.
/// The runtime kernel is responsible for applying the response.
pub trait Mode: Send + Sync {
    fn on_session_start(
        &self,
        session: &Session,
        env: &Envelope,
    ) -> Result<ModeResponse, MacpError>;

    fn on_message(&self, session: &Session, env: &Envelope) -> Result<ModeResponse, MacpError>;
}
