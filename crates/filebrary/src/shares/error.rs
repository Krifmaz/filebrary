//! Errors for share routes.
//!
//! Most failures reuse [`AppError`]; the few share-specific outcomes (a
//! password is needed, the download limit is used up) get their own codes but
//! keep the same `{"error", "message"}` body.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

use crate::{error::AppError, storage::StorageError};

/// Error returned by share handlers.
#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    /// The share has a password and the visitor has not unlocked it.
    #[error("this share is protected by a password")]
    PasswordRequired,
    /// The password sent to the unlock endpoint is wrong.
    #[error("wrong password")]
    WrongPassword,
    /// The share's download limit has been reached.
    #[error("this share has reached its download limit")]
    Exhausted,
    /// Any other error, reported the usual way.
    #[error(transparent)]
    App(#[from] AppError),
}

impl ShareError {
    /// Shorthand for an internal error with a message that is logged, not shown.
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::App(AppError::Internal(anyhow::anyhow!(msg.into())))
    }

    /// Shorthand for a 400 with `msg`.
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::App(AppError::bad_request(msg))
    }

    /// Unknown, expired or revoked share. Deliberately one answer for all of
    /// these so tokens cannot be probed.
    pub fn not_found() -> Self {
        Self::App(AppError::NotFound)
    }
}

impl From<sqlx::Error> for ShareError {
    fn from(e: sqlx::Error) -> Self {
        Self::App(e.into())
    }
}

impl From<StorageError> for ShareError {
    fn from(e: StorageError) -> Self {
        Self::App(e.into())
    }
}

impl IntoResponse for ShareError {
    fn into_response(self) -> Response {
        let (status, code, needs_password) = match &self {
            Self::PasswordRequired => (StatusCode::UNAUTHORIZED, "password_required", true),
            Self::WrongPassword => (StatusCode::UNAUTHORIZED, "wrong_password", true),
            Self::Exhausted => (StatusCode::GONE, "download_limit_reached", false),
            Self::App(_) => {
                let Self::App(e) = self else {
                    unreachable!("matched above")
                };
                return e.into_response();
            }
        };
        let mut body = json!({ "error": code, "message": self.to_string() });
        if needs_password {
            body["needs_password"] = json!(true);
        }
        (status, Json(body)).into_response()
    }
}

/// Result alias for share handlers.
pub type ShareResult<T> = Result<T, ShareError>;
