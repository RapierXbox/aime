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

    #[error("IO error")]
    IOError,

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

    #[error("SerdeJson error")]
    SerdeJson,

    #[error("Account not found for id")]
    AccountNotFound,

    #[error("Gmail error: {0}")]
    GmailErr(#[from] gmail::GmailError),

    #[error("Gmail API error: {0}")]
    GmailApiErr(#[from] gmail::GmailApiError),

    #[error("could not parse account id")]
    ParseAccountID,

    #[error("could not parse email body")]
    InvalidEmailBody,
}
