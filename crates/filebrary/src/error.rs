//! Error type shared by the HTTP layer and the services below it.
//!
//! Every handler returns [`AppResult`]. Errors are converted into a JSON body
//! of the form `{"error": "<code>", "message": "<human text>"}` with a matching
//! HTTP status. Internal errors are logged with full detail and reported to the
//! client with a generic message so that paths and SQL never leak.

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::storage::StorageError;

/// Convenience alias for results produced by request handlers and services.
pub type AppResult<T> = Result<T, AppError>;

/// An error that can be turned into an HTTP response.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// The request was malformed or failed validation.
    #[error("{0}")]
    BadRequest(String),
    /// No valid session or credentials were supplied.
    #[error("authentication required")]
    Unauthorized,
    /// A login named an unknown user, gave a wrong password, or named a
    /// disabled account. The cases are deliberately indistinguishable.
    #[error("invalid username or password")]
    InvalidCredentials,
    /// The password was right but the account needs a second factor
    /// (a TOTP code) that the login did not include.
    #[error("a two-factor code is required")]
    TotpRequired,
    /// The caller is authenticated but lacks the permission needed.
    #[error("{0}")]
    Forbidden(String),
    /// The requested resource does not exist (or is hidden from the caller).
    #[error("not found")]
    NotFound,
    /// The request conflicts with the current state (for example the target exists).
    #[error("{0}")]
    Conflict(String),
    /// The resource existed but is no longer available (expired share link).
    #[error("{0}")]
    Gone(String),
    /// The requested byte range cannot be satisfied.
    #[error("range not satisfiable")]
    RangeNotSatisfiable {
        /// Total size of the resource, sent back in `Content-Range`.
        size: u64,
    },
    /// The payload is larger than the server accepts.
    #[error("payload too large")]
    PayloadTooLarge,
    /// The caller exceeded a rate limit and must wait.
    #[error("too many attempts, retry in {retry_after} seconds")]
    TooManyRequests {
        /// Seconds until the caller may try again.
        retry_after: u64,
    },
    /// An unexpected failure. The inner error is logged, never shown.
    #[error("internal error")]
    Internal(#[from] anyhow::Error),
}

impl AppError {
    /// Shorthand for [`AppError::BadRequest`].
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }

    /// Shorthand for [`AppError::Forbidden`].
    pub fn forbidden(msg: impl Into<String>) -> Self {
        Self::Forbidden(msg.into())
    }

    /// Shorthand for [`AppError::Conflict`].
    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid_credentials"),
            Self::TotpRequired => (StatusCode::UNAUTHORIZED, "totp_required"),
            Self::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            Self::Gone(_) => (StatusCode::GONE, "gone"),
            Self::RangeNotSatisfiable { .. } => {
                (StatusCode::RANGE_NOT_SATISFIABLE, "range_not_satisfiable")
            }
            Self::PayloadTooLarge => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            Self::TooManyRequests { .. } => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        }
    }
}

impl From<StorageError> for AppError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::NotFound => Self::NotFound,
            StorageError::AlreadyExists => Self::conflict("the target already exists"),
            StorageError::InvalidPath(m) => Self::BadRequest(format!("invalid path: {m}")),
            StorageError::OutsideRoot => Self::forbidden("path is outside the source"),
            StorageError::NotADirectory => Self::bad_request("not a directory"),
            StorageError::IsADirectory => Self::bad_request("is a directory"),
            StorageError::InvalidRange { size } => Self::RangeNotSatisfiable { size },
            StorageError::Unsupported(m) => Self::bad_request(m),
            StorageError::Io(e) => Self::Internal(anyhow::Error::new(e)),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::NotFound,
            other => Self::Internal(anyhow::Error::new(other)),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::Internal(anyhow::Error::new(e))
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = match &self {
            Self::Internal(inner) => {
                tracing::error!(error = ?inner, "internal error");
                "internal error".to_owned()
            }
            other => other.to_string(),
        };
        let mut resp = (status, Json(json!({ "error": code, "message": message }))).into_response();
        match self {
            Self::TooManyRequests { retry_after } => {
                if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
                    resp.headers_mut().insert(header::RETRY_AFTER, v);
                }
            }
            Self::RangeNotSatisfiable { size } => {
                if let Ok(v) = HeaderValue::from_str(&format!("bytes */{size}")) {
                    resp.headers_mut().insert(header::CONTENT_RANGE, v);
                }
            }
            _ => {}
        }
        resp
    }
}
