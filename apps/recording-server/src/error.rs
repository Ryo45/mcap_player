use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ErrorKind {
    BadRequest,
    NotFound,
    Conflict,
    TooLarge,
    Unprocessable,
    Internal,
}

#[derive(Debug)]
pub(crate) struct ServerError {
    pub(crate) kind: ErrorKind,
    pub(crate) code: &'static str,
    public_message: String,
    source_message: Option<String>,
}

impl ServerError {
    pub(crate) fn bad_request(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::BadRequest, code, message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, "recording_not_found", message)
    }

    pub(crate) fn conflict(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Conflict, "revision_mismatch", message)
    }

    pub(crate) fn too_large(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::TooLarge, code, message)
    }

    pub(crate) fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unprocessable, code, message)
    }

    pub(crate) fn internal(message: impl Into<String>, source: impl fmt::Display) -> Self {
        let mut error = Self::new(ErrorKind::Internal, "internal_error", message);
        error.source_message = Some(source.to_string());
        error
    }

    fn new(kind: ErrorKind, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            code,
            public_message: message.into(),
            source_message: None,
        }
    }

    pub(crate) fn status(&self) -> StatusCode {
        match self.kind {
            ErrorKind::BadRequest => StatusCode::BAD_REQUEST,
            ErrorKind::NotFound => StatusCode::NOT_FOUND,
            ErrorKind::Conflict => StatusCode::CONFLICT,
            ErrorKind::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
            ErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.public_message)?;
        if let Some(source) = &self.source_message {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl Error for ServerError {}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

impl IntoResponse for ServerError {
    fn into_response(self) -> axum::response::Response {
        let status = self.status();
        if let Some(source) = &self.source_message {
            tracing::error!(code = self.code, error = %source, "request failed");
        }
        (
            status,
            Json(ErrorBody {
                code: self.code,
                message: &self.public_message,
            }),
        )
            .into_response()
    }
}
