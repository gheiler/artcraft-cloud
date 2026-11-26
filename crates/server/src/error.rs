use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use artcraft_cloud_types::{ErrorBody, TypeConflict};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not found")]
    NotFound,
    #[error("already exists")]
    Exists,
    #[error("etag conflict")]
    Conflict { etag: String },
    #[error("forbidden")]
    Forbidden,
    #[error("invalid path")]
    BadPath,
    #[error("{0}")]
    Invalid(String),
    #[error("file is too large")]
    TooLarge,
    #[error("unauthorized")]
    Unauthorized,
    #[error("database is busy")]
    Locked,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
}

pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub etag: Option<String>,
    pub code: Option<String>,
    pub conflicts: Vec<TypeConflict>,
}

impl ApiError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self { status, message: message.into(), etag: None, code: None, conflicts: Vec::new() }
    }

    pub fn with_conflicts(status: StatusCode, message: impl Into<String>, code: &str, conflicts: Vec<TypeConflict>) -> Self {
        Self { status, message: message.into(), etag: None, code: Some(code.into()), conflicts }
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized")
    }
}

impl From<Error> for ApiError {
    fn from(err: Error) -> Self {
        match err {
            Error::NotFound => Self::new(StatusCode::NOT_FOUND, "not found"),
            Error::Exists => Self::new(StatusCode::CONFLICT, "already exists"),
            Error::Conflict { etag } => Self {
                status: StatusCode::PRECONDITION_FAILED,
                message: "etag conflict".into(),
                etag: Some(etag),
                code: None,
                conflicts: Vec::new(),
            },
            Error::Forbidden => Self::new(StatusCode::FORBIDDEN, "forbidden"),
            Error::BadPath => Self::new(StatusCode::BAD_REQUEST, "invalid path"),
            Error::Invalid(message) => Self::new(StatusCode::BAD_REQUEST, message),
            Error::TooLarge => Self::new(StatusCode::PAYLOAD_TOO_LARGE, "file is too large"),
            Error::Unauthorized => Self::unauthorized(),
            Error::Locked => Self::new(StatusCode::SERVICE_UNAVAILABLE, "database is busy"),
            Error::Io(err) => {
                tracing::error!(%err, "storage");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "storage error")
            }
            Error::Db(err) => {
                tracing::error!(%err, "database");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "database error")
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status;
        let body = Json(ErrorBody { error: self.message, etag: self.etag, code: self.code, conflicts: self.conflicts });
        (status, body).into_response()
    }
}
