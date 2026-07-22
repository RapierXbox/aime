//! provides error types

use std::{fs, num::NonZeroU16};

use log::{debug, error};
use serde::Serialize;
use specta::Type;
use thiserror::Error;

use crate::email::gmail;

/// Machine-readable database constraint kind, mirrors `sqlx::error::ErrorKind`.
#[derive(Error, Debug, Serialize, Type)]
pub enum SqlxDbErrorKind {
    #[error("unique violation")]
    UniqueViolation,
    #[error("foreign key violation")]
    ForeignKeyViolation,
    #[error("not null violation")]
    NotNullViolation,
    #[error("check violation")]
    CheckViolation,
    #[error("other")]
    Other,
}

/// Typed wrapper around `sqlx::Error` with machine-readable variants.
/// No string fields — use the error `Display` for human messages.
#[derive(Error, Debug, Serialize, Type)]
pub enum SqlxError {
    #[error("row not found")]
    RowNotFound,
    /// Database-level error; `kind` encodes the constraint violation category.
    #[error("database error: {kind}")]
    Database { kind: SqlxDbErrorKind },
    #[error("column index {index} out of bounds (len {len})")]
    ColumnIndexOutOfBounds { index: u32, len: u32 },
    #[error("pool timed out")]
    PoolTimedOut,
    #[error("pool closed")]
    PoolClosed,
    #[error("worker crashed")]
    WorkerCrashed,
    #[error("other")]
    Other,
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        SqlxError::from(value).into()
    }
}

impl From<sqlx::Error> for SqlxError {
    fn from(e: sqlx::Error) -> Self {
        match e {
            sqlx::Error::RowNotFound => Self::RowNotFound,
            sqlx::Error::Database(e) => Self::Database {
                kind: match e.kind() {
                    sqlx::error::ErrorKind::UniqueViolation => SqlxDbErrorKind::UniqueViolation,
                    sqlx::error::ErrorKind::ForeignKeyViolation => {
                        SqlxDbErrorKind::ForeignKeyViolation
                    }
                    sqlx::error::ErrorKind::NotNullViolation => SqlxDbErrorKind::NotNullViolation,
                    sqlx::error::ErrorKind::CheckViolation => SqlxDbErrorKind::CheckViolation,
                    _ => SqlxDbErrorKind::Other,
                },
            },
            sqlx::Error::ColumnIndexOutOfBounds { index, len } => Self::ColumnIndexOutOfBounds {
                index: index as u32,
                len: len as u32,
            },
            sqlx::Error::PoolTimedOut => Self::PoolTimedOut,
            sqlx::Error::PoolClosed => Self::PoolClosed,
            sqlx::Error::WorkerCrashed => Self::WorkerCrashed,
            _ => Self::Other,
        }
    }
}

/// Machine-readable subset of `std::io::ErrorKind`; rest collapse to `Other`.
#[derive(Error, Debug, Serialize, Type)]
pub enum IoError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("already exists")]
    AlreadyExists,
    #[error("connection failed")]
    Connection,
    #[error("timed out")]
    TimedOut,
    #[error("unexpected eof")]
    UnexpectedEof,
    #[error("other")]
    Other,
}

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        match e.kind() {
            K::NotFound => Self::NotFound,
            K::PermissionDenied => Self::PermissionDenied,
            K::AlreadyExists => Self::AlreadyExists,
            K::ConnectionRefused | K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe => {
                Self::Connection
            }
            K::TimedOut => Self::TimedOut,
            K::UnexpectedEof => Self::UnexpectedEof,
            _ => Self::Other,
        }
    }
}

/// Mirrors `serde_json::error::Category` — the meaningful failure kinds.
#[derive(Error, Debug, Serialize, Type)]
pub enum SerdeError {
    #[error("io")]
    Io,
    #[error("syntax")]
    Syntax,
    #[error("data")]
    Data,
    #[error("eof")]
    Eof,
}

impl From<serde_json::Error> for SerdeError {
    fn from(e: serde_json::Error) -> Self {
        use serde_json::error::Category as C;
        match e.classify() {
            C::Io => Self::Io,
            C::Syntax => Self::Syntax,
            C::Data => Self::Data,
            C::Eof => Self::Eof,
        }
    }
}

/// Tauri runtime errors. `Io`/`Json` are peeled off to the top-level `AppError`
/// variants (same failure, one canonical code); everything else → `Other`.
/// `tauri::Error` is `#[non_exhaustive]` and huge — only the codes we act on.
#[derive(Error, Debug, Serialize, Type)]
pub enum TauriError {
    #[error("invalid command args")]
    InvalidArgs,
    #[error("setup hook failed")]
    Setup,
    #[error("other")]
    Other,
}

impl From<tauri::Error> for TauriError {
    fn from(e: tauri::Error) -> Self {
        match e {
            tauri::Error::InvalidArgs(..) => Self::InvalidArgs,
            tauri::Error::Setup(..) => Self::Setup,
            _ => Self::Other,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        IoError::from(e).into()
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        SerdeError::from(e).into()
    }
}

impl From<tauri::Error> for AppError {
    fn from(e: tauri::Error) -> Self {
        // io/json aren't tauri-specific: route them to their canonical variant.
        match e {
            tauri::Error::Io(io) => IoError::from(io).into(),
            tauri::Error::Json(j) => SerdeError::from(j).into(),
            other => TauriError::from(other).into(),
        }
    }
}

#[derive(Error, Debug, Serialize, Type)]
pub enum AppError {
    #[error("Missing database File")]
    MissingDbPath,

    #[error("Gmail api response incomplete")]
    GmailResponseIncomplete,

    #[error("OAuth error")]
    OAuth,

    #[error("Http status: {0}")]
    HttpErr(NonZeroU16),

    #[error("sqlx error")]
    Sqlx(#[from] SqlxError),

    #[error("IO error: {0}")]
    Io(#[from] IoError),

    #[error("Url parse error")]
    UrlParseError,

    #[error("Failed to save credentials to keyring")]
    KeyringSaveError,

    #[error("Failed to load credentials from keyring")]
    KeyringLoadError,

    #[error("Expected different account type")]
    AccountTypeMismatch,

    #[error("Gmail did not return labels")]
    GmailMissingLabels,

    #[error("SerdeJson error: {0}")]
    Serde(#[from] SerdeError),

    #[error("Tauri error: {0}")]
    Tauri(#[from] TauriError),

    #[error("Account not found for id")]
    AccountNotFound,

    #[error("Gmail error: {0}")]
    GmailErr(#[from] gmail::GmailError),

    #[error("Gmail API error: {0}")]
    GmailApiErr(#[from] gmail::GmailApiError),

    #[error("could not parse account id")]
    ParseAccountID,

    #[error("could not parse sync cursor")]
    ParseSyncCursor,

    #[error("could not parse email body")]
    InvalidEmailBody,
}
