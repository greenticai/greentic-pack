use std::io;

#[cfg(feature = "http-runtime")]
use reqwest::StatusCode;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DistributorError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[cfg(feature = "http-runtime")]
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("wit error: {0}")]
    Wit(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("resource not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[cfg(feature = "http-runtime")]
    #[error("unexpected status {status}: {body}")]
    Status { status: StatusCode, body: String },
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("other distributor error: {0}")]
    Other(String),
}
