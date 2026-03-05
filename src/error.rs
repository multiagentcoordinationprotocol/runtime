use thiserror::Error;

#[derive(Debug, Error)]
pub enum MacpError {
    #[error("InvalidMacpVersion")]
    InvalidMacpVersion,
    #[error("InvalidEnvelope")]
    InvalidEnvelope,
    #[error("DuplicateSession")]
    DuplicateSession,
    #[error("UnknownSession")]
    UnknownSession,
    #[error("SessionNotOpen")]
    SessionNotOpen,
    #[error("TtlExpired")]
    TtlExpired,
    #[error("InvalidTtl")]
    InvalidTtl,
    #[error("UnknownMode")]
    UnknownMode,
    #[error("InvalidModeState")]
    InvalidModeState,
    #[error("InvalidPayload")]
    InvalidPayload,
}
