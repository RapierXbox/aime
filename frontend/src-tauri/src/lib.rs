use std::fs;

use google_gmail1::hyper_util::rt::tokio;
use log::error;
use rand::distr::{Alphanumeric, SampleString};
use serde::Serialize;
use tauri::{async_runtime, generate_handler, http::StatusCode, ipc::IpcResponse, App, Manager};
use tauri_plugin_keyring::KeyringExt;
use thiserror::Error;

mod email;

pub const KEYRING_SERVICE: &str = "aime";

#[derive(Error, Debug, Serialize)]
pub enum Error {
    #[error("Missing database File")]
    MissingDbPath,

    #[error("Gmail error: {0}")]
    #[serde(serialize_with = "ser_as_string")]
    Gmail(#[from] google_gmail1::Error),

    #[error("Gmail api response incomplete")]
    GmailIncomplete,

    #[error("OAuth error")]
    OAuth,

    #[error("Http status: {0}")]
    #[serde(serialize_with = "ser_as_string")]
    HttpErr(StatusCode),

    #[error(transparent)]
    #[serde(serialize_with = "ser_as_string")]
    SQLXError(#[from] sqlx::Error),

    #[error(transparent)]
    #[serde(serialize_with = "ser_as_string")]
    IOError(#[from] std::io::Error),

    #[error(transparent)]
    #[serde(serialize_with = "ser_as_string")]
    UrlParseError(#[from] oauth2::url::ParseError),

    #[error("Failed to save credentials to keyring")]
    KeyringSaveError,

    #[error("Failed to load credentials from keyring")]
    KeyringLoadError,

    #[error("Expected different account type")]
    AccountTypeMismatch,

    #[error("Gmail did not return labels")]
    GmailMissingLabels,
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
    tauri::Builder::default()
        .plugin(tauri_plugin_keyring::init())
        .setup(setup)
        .invoke_handler(generate_handler![
            email::gmail::register_gmail_account,
            email::email_list_accounts,
            email::dev_do_onboard_sync
        ])
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

    let path_str = sqlx_dir.to_str().ok_or(Error::MissingDbPath)?;

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
