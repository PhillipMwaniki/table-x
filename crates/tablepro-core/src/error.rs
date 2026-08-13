//! Unified error type.
//!
//! Driver errors are normalized here so the UI can react to *categories* of failure
//! (auth, network, syntax) without pattern-matching on vendor-specific error strings.

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("connection failed: {0}")]
    Connection(String),

    #[error("authentication failed: {0}")]
    Auth(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("TLS error: {0}")]
    Tls(String),

    #[error("SSH tunnel error: {0}")]
    Tunnel(String),

    /// A syntax or semantic error in user-supplied SQL. Carries position
    /// information when the database reports it, so the editor can underline
    /// the offending token instead of just showing a message.
    #[error("query error: {message}")]
    Query {
        message: String,
        /// 1-based character offset into the statement, if known.
        position: Option<u32>,
        /// Vendor error code (e.g. PostgreSQL `SQLSTATE`, MySQL errno).
        code: Option<String>,
    },

    #[error("query was cancelled")]
    Cancelled,

    #[error("timed out after {0}s")]
    Timeout(u64),

    #[error("no such connection: {0}")]
    UnknownConnection(String),

    #[error("driver '{0}' is not registered")]
    UnknownDriver(String),

    /// The operation is legitimate but this database cannot do it — for example
    /// editing the result of a join, which has no single underlying table.
    #[error("unsupported: {0}")]
    Unsupported(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    #[error("serialization error: {0}")]
    Serde(String),

    #[error("I/O error: {0}")]
    Io(String),

    #[error("{0}")]
    Other(String),
}

impl Error {
    /// Category used by the UI to choose an icon, tone, and recovery action.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Error::Auth(_) => ErrorCategory::Auth,
            Error::Connection(_) | Error::Network(_) | Error::Tls(_) | Error::Tunnel(_) => {
                ErrorCategory::Connection
            }
            Error::Query { .. } => ErrorCategory::Query,
            Error::Cancelled => ErrorCategory::Cancelled,
            Error::Timeout(_) => ErrorCategory::Timeout,
            Error::Unsupported(_) => ErrorCategory::Unsupported,
            Error::Config(_) | Error::UnknownConnection(_) | Error::UnknownDriver(_) => {
                ErrorCategory::Config
            }
            _ => ErrorCategory::Internal,
        }
    }

    /// Whether retrying the same operation could plausibly succeed.
    /// Drives whether the UI offers a "Retry" button.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Network(_) | Error::Timeout(_) | Error::Connection(_)
        )
    }

    pub fn query(message: impl Into<String>) -> Self {
        Error::Query {
            message: message.into(),
            position: None,
            code: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Connection,
    Auth,
    Query,
    Cancelled,
    Timeout,
    Unsupported,
    Config,
    Internal,
}

/// Wire representation sent to the frontend. `Error` itself is not `Serialize`
/// because `thiserror` sources are not, so we flatten it at the boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub message: String,
    pub category: ErrorCategory,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl From<&Error> for ErrorPayload {
    fn from(e: &Error) -> Self {
        let (position, code) = match e {
            Error::Query { position, code, .. } => (*position, code.clone()),
            _ => (None, None),
        };
        ErrorPayload {
            message: e.to_string(),
            category: e.category(),
            retryable: e.is_retryable(),
            position,
            code,
        }
    }
}

impl From<Error> for ErrorPayload {
    fn from(e: Error) -> Self {
        ErrorPayload::from(&e)
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        ErrorPayload::from(self).serialize(s)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Serde(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn network_failures_offer_retry_but_auth_failures_do_not() {
        assert!(Error::Network("reset".into()).is_retryable());
        // Retrying identical bad credentials just burns login attempts.
        assert!(!Error::Auth("bad password".into()).is_retryable());
    }

    #[test]
    fn query_errors_carry_position_to_the_editor() {
        let e = Error::Query {
            message: "syntax error at or near \"slect\"".into(),
            position: Some(1),
            code: Some("42601".into()),
        };
        let payload = ErrorPayload::from(&e);
        assert_eq!(payload.category, ErrorCategory::Query);
        assert_eq!(payload.position, Some(1));
        assert_eq!(payload.code.as_deref(), Some("42601"));
    }

    #[test]
    fn errors_serialize_flat_for_the_frontend() {
        let json = serde_json::to_value(Error::Timeout(30)).expect("serialize");
        assert_eq!(json["category"], "timeout");
        assert_eq!(json["retryable"], true);
        assert_eq!(json["message"], "timed out after 30s");

        // A statement the database simply cannot perform is not worth retrying.
        let json = serde_json::to_value(Error::Unsupported("joins are not editable".into()))
            .expect("serialize");
        assert_eq!(json["retryable"], false);
    }
}
