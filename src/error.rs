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
    #[error("Forbidden")]
    Forbidden,
    #[error("Unauthenticated")]
    Unauthenticated,
    /// Kept for RFC error-code completeness.  The runtime currently represents
    /// duplicate detection via `ProcessResult { duplicate: true }` at the Ack
    /// level rather than returning this as an error.
    #[error("DuplicateMessage")]
    DuplicateMessage,
    #[error("PayloadTooLarge")]
    PayloadTooLarge,
    #[error("RateLimited")]
    RateLimited,
}

impl MacpError {
    /// Returns the RFC error code string for this error variant.
    pub fn error_code(&self) -> &'static str {
        match self {
            MacpError::InvalidMacpVersion => "UNSUPPORTED_PROTOCOL_VERSION",
            MacpError::InvalidEnvelope => "INVALID_ENVELOPE",
            MacpError::DuplicateSession => "INVALID_ENVELOPE",
            MacpError::UnknownSession => "SESSION_NOT_FOUND",
            MacpError::SessionNotOpen => "SESSION_NOT_OPEN",
            MacpError::TtlExpired => "SESSION_NOT_OPEN",
            MacpError::InvalidTtl => "INVALID_ENVELOPE",
            MacpError::UnknownMode => "MODE_NOT_SUPPORTED",
            MacpError::InvalidModeState => "INVALID_ENVELOPE",
            MacpError::InvalidPayload => "INVALID_ENVELOPE",
            MacpError::Forbidden => "FORBIDDEN",
            MacpError::Unauthenticated => "UNAUTHENTICATED",
            MacpError::DuplicateMessage => "DUPLICATE_MESSAGE",
            MacpError::PayloadTooLarge => "PAYLOAD_TOO_LARGE",
            MacpError::RateLimited => "RATE_LIMITED",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_mapping_covers_all_variants() {
        let cases: Vec<(MacpError, &str)> = vec![
            (
                MacpError::InvalidMacpVersion,
                "UNSUPPORTED_PROTOCOL_VERSION",
            ),
            (MacpError::InvalidEnvelope, "INVALID_ENVELOPE"),
            (MacpError::DuplicateSession, "INVALID_ENVELOPE"),
            (MacpError::UnknownSession, "SESSION_NOT_FOUND"),
            (MacpError::SessionNotOpen, "SESSION_NOT_OPEN"),
            (MacpError::TtlExpired, "SESSION_NOT_OPEN"),
            (MacpError::InvalidTtl, "INVALID_ENVELOPE"),
            (MacpError::UnknownMode, "MODE_NOT_SUPPORTED"),
            (MacpError::InvalidModeState, "INVALID_ENVELOPE"),
            (MacpError::InvalidPayload, "INVALID_ENVELOPE"),
            (MacpError::Forbidden, "FORBIDDEN"),
            (MacpError::Unauthenticated, "UNAUTHENTICATED"),
            (MacpError::DuplicateMessage, "DUPLICATE_MESSAGE"),
            (MacpError::PayloadTooLarge, "PAYLOAD_TOO_LARGE"),
            (MacpError::RateLimited, "RATE_LIMITED"),
        ];

        for (error, expected_code) in cases {
            assert_eq!(
                error.error_code(),
                expected_code,
                "error_code() mismatch for {:?}",
                error
            );
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn display_matches_variant_name() {
        assert_eq!(
            MacpError::InvalidMacpVersion.to_string(),
            "InvalidMacpVersion"
        );
        assert_eq!(MacpError::Forbidden.to_string(), "Forbidden");
        assert_eq!(MacpError::TtlExpired.to_string(), "TtlExpired");
        assert_eq!(MacpError::Unauthenticated.to_string(), "Unauthenticated");
        assert_eq!(MacpError::DuplicateMessage.to_string(), "DuplicateMessage");
        assert_eq!(MacpError::PayloadTooLarge.to_string(), "PayloadTooLarge");
        assert_eq!(MacpError::RateLimited.to_string(), "RateLimited");
    }
}
