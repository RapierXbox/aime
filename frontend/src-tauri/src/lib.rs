use std::{fs, time::Duration};

use log::{debug, error};
use serde::Serialize;
use specta::Type;

use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions};
use tauri::{async_runtime, App, Manager};
use tauri_specta::{collect_commands, Builder};
use thiserror::Error;

#[cfg(debug_assertions)]
use specta_typescript::Typescript;

mod email;
mod secret_store;

use email::gmail;

/// The keyring service name used for storing Gmail refresh tokens.
pub const KEYRING_SERVICE: &str = "aime";

pub mod error;
pub use error::AppError;

use crate::error::SqlxError;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![
            gmail::register_gmail_account,
            email::email_list_accounts,
            email::dev_email_full_sync,
            email::email_sync
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

    debug!("db_path={path_str:?}");

    // connect_lazy braucht ein async context, daher block_on
    let pres: Result<_, AppError> = async_runtime::block_on(async {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(SqliteConnectOptions::new().filename(sqlx_dir));

        sqlx::migrate!().run(&pool).await.map_err(|e| {
            error!("Failed to run migrations: {e:?}");
            SqlxError::Other
        })?;
        Ok(pool)
    });

    let pool = pres?;

    // setup the email handling
    email::setup(app, &pool)?;

    app.manage(pool);

    Ok(())
}
