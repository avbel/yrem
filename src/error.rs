//! Library error type. Every variant maps to a process exit code.

use std::process::ExitCode;

/// Errors returned by the client and the operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 401 / 403.
    #[error("authentication failed (HTTP {status}): {message}")]
    Auth { status: u16, message: String },
    /// 404.
    #[error("not found: {0}")]
    NotFound(String),
    /// 429.
    #[error("quota exceeded: {0}")]
    Quota(String),
    /// 501: the server has no HTTP path for this operation.
    #[error("server does not support `{operation}` over HTTP: {reason}")]
    Unsupported { operation: String, reason: String },
    /// 503, timeouts and unreachable servers.
    #[error("server unavailable: {0}")]
    Unavailable(String),
    /// Any other non-2xx status.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    /// Transport failure other than timeout / connect.
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The server answered 2xx with a body that is not the expected JSON.
    #[error("invalid response from server: {0}")]
    Decode(String),
    /// Bad arguments, rejected before any request is sent.
    #[error("invalid argument: {0}")]
    Invalid(String),
    /// Emulated `correct` wrote the replacement but could not forget the old record.
    #[error(
        "correct wrote new memory {new_rid} but failed to forget {old_rid} ({source}); retry forget(\"{old_rid}\")"
    )]
    PartialCorrect {
        old_rid: String,
        new_rid: String,
        source: Box<Error>,
    },
}

impl Error {
    /// Process exit code for this error (see README, "Output and exit codes").
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Invalid(_) => 2,
            Self::Auth { .. } => 3,
            Self::Unsupported { .. } => 4,
            Self::Unavailable(_) => 5,
            Self::PartialCorrect { .. } => 6,
            Self::NotFound(_)
            | Self::Quota(_)
            | Self::Http { .. }
            | Self::Transport(_)
            | Self::Decode(_) => 1,
        }
    }

    /// Whether repeating the same request may succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Quota(_) | Self::Unavailable(_) | Self::Transport(_) => true,
            Self::Http { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

impl From<&Error> for ExitCode {
    fn from(e: &Error) -> Self {
        Self::from(e.exit_code())
    }
}

/// Library result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;
