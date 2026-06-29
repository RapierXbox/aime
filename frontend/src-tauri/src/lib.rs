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

    #[error("SQLX error")]
    SQLXError,

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

    #[error("Gmail error: {0}")]
    GmailErr(#[from] gmail::GmailError),

    #[error("could not parse account id")]
    ParseAccountID,
}

fn ser_as_string<T, S>(err: T, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
    T: std::fmt::Debug,
{
    error!("Error returned from tauri: {:?}", err);
    serializer.serialize_str(&format!("{:?}", err))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = Builder::<tauri::Wry>::new()
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
