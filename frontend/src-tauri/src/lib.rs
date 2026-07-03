use std::{fs, num::NonZeroU16};

use log::error;
use serde::Serialize;
use specta::Type;

use tauri::{async_runtime, App, Manager};
use tauri_specta::{collect_commands, Builder};
use thiserror::Error;

#[cfg(debug_assertions)]
use specta_typescript::Typescript;

mod email;
use email::gmail;

pub const KEYRING_SERVICE: &str = "aime";

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![
            gmail::register_gmail_account,
            email::email_list_accounts,
            email::dev_email_full_sync
        ]);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    tauri::Builder::default()
        .plugin(tauri_plugin_keyring::init())
        .setup(setup)
        .invoke_handler(builder.invoke_handler())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub type DbPool = sqlx::sqlite::SqlitePool;

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    if cfg!(debug_assertions) {
        app.handle().plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .build(),
        )?;
    }

    let sqlx_dir = app.path().app_data_dir()?.join("aime.sqlite");
    if let Some(parent) = sqlx_dir.parent() {
        fs::create_dir_all(parent)?;
    }

    let path_str = sqlx_dir.to_str().ok_or(AppError::MissingDbPath)?;

    // connect_lazy braucht ein async context, daher block_on
    let pool = async_runtime::block_on(async {
        let pres = sqlx::sqlite::SqlitePool::connect_lazy(path_str);
        match pres {
            Ok(pool) => {
                sqlx::migrate!().run(&pool).await?;
                Ok(pool)
            }
            Err(e) => Err(e),
        }
    })?;

    // setup the email handling
    email::setup(app, &pool)?;

    app.manage(pool);

    Ok(())
}
