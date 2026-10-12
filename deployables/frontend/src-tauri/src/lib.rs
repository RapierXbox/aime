#![allow(unused)]

use crate::error::SqlxError;
use ecow::EcoString;
use email::gmail;
use log::{debug, error, warn};
use serde::Serialize;
use specta::Type;
use sqlx::{sqlite::SqliteConnectOptions, ConnectOptions};
use std::{fs, time::Duration};
use tauri::{async_runtime, ipc::Channel, App, Manager};
use tauri_specta::{collect_commands, collect_events, Builder};
use thiserror::Error;

#[cfg(debug_assertions)]
use specta_typescript::Typescript;

mod email;
pub mod error;
mod secret_store;

pub use error::AppError;

/// The keyring service name used for storing Gmail refresh tokens.
pub const KEYRING_SERVICE: &str = "aime";

// TODO: maybe create a more sophisticated progress tracker
// where you can create sections and Updates show inside those sections so progress doesnt jump around

#[derive(specta::Type, Debug, Clone, Serialize)]
pub enum Progress {
    Update {
        #[specta(type = specta_typescript::Number)]
        completed: u64,
        #[specta(type = specta_typescript::Number)]
        out_of: Option<u64>,
    },
    Message(IpcEcoString),
}

pub trait ProgressReporter {
    fn report(&self, update: Progress);
    fn report_message(&self, message: EcoString);
}

/// Tells the frontend which cached queries are stale.
/// Message-level events cover all messages of an account, not single messages.
#[derive(specta::Type, Debug, Clone, PartialEq, Serialize, tauri_specta::Event)]
pub enum InvalidateEvent {
    GetMessage { account_id: String },
    ListMessages { account_id: String },
    Accounts,
}

impl InvalidateEvent {
    /// push `self` unless already present
    // ponytail: linear search, fine for the handful of events a sync produces
    pub fn add_to(self, events: &mut Vec<InvalidateEvent>) {
        if !events.contains(&self) {
            events.push(self);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = Builder::<tauri::Wry>::new()
        // Then register them (separated by a comma)
        .commands(collect_commands![
            gmail::register_gmail_account,
            email::email_list_accounts,
            email::dev_email_full_sync,
            email::email_sync,
            email::list_messages,
            email::get_message
        ])
        .events(collect_events![InvalidateEvent]);

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    builder
        .export(Typescript::default(), "../src/bindings.ts")
        .expect("Failed to export typescript bindings");

    let invoke_handler = builder.invoke_handler();

    tauri::Builder::default()
        .plugin(tauri_plugin_keyring::init())
        // run the setup fn below
        .setup(move |app| {
            builder.mount_events(app);
            setup(app)
        })
        .invoke_handler(invoke_handler)
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub type DbPool = sqlx::sqlite::SqlitePool;

fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    // add level filter info  for debug builds
    if cfg!(debug_assertions) {
        app.handle().plugin(
            tauri_plugin_log::Builder::default()
                .level(log::LevelFilter::Info)
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "[{} {} {} {}:{}] {}",
                        chrono::Local::now().format("%H:%M:%S%.3f"),
                        record.level(),
                        record.target(),
                        record.file().unwrap_or("?"),
                        record.line().unwrap_or(0),
                        message
                    ))
                })
                .build(),
        )?;
    }

    // --- db setup
    let sqlx_dir = app.path().app_data_dir()?.join("aime.sqlite");
    if let Some(parent) = sqlx_dir.parent() {
        fs::create_dir_all(parent)?;
    }
    let path_str = sqlx_dir.to_str().ok_or(AppError::MissingDbPath)?;
    debug!("db_path={path_str:?}");

    // connect_lazy needs an async context
    let pres: Result<_, AppError> = async_runtime::block_on(async {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs(5))
            .connect_lazy_with(
                SqliteConnectOptions::new()
                    .filename(sqlx_dir)
                    .optimize_on_close(true, None),
            );

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

impl ProgressReporter for Channel<Progress> {
    fn report(&self, update: Progress) {
        self.send(update)
            .inspect_err(|e| warn!("failed to report progress: {e:?}"))
            .ok();
    }

    fn report_message(&self, message: EcoString) {
        self.send(Progress::Message(IpcEcoString(message)))
            .inspect_err(|e| warn!("failed to report message: {e:?}"))
            .ok();
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct IpcEcoString(pub EcoString);

impl Type for IpcEcoString {
    // just a string over the wire
    fn definition(types: &mut specta::Types) -> specta::datatype::DataType {
        String::definition(types)
    }
}

#[cfg(test)]
mod tests {
    use super::InvalidateEvent;

    #[test]
    fn invalidate_event_add_to_dedups() {
        let mut events = vec![];
        InvalidateEvent::Accounts.add_to(&mut events);
        InvalidateEvent::Accounts.add_to(&mut events);
        InvalidateEvent::ListMessages {
            account_id: "1".into(),
        }
        .add_to(&mut events);
        InvalidateEvent::ListMessages {
            account_id: "2".into(),
        }
        .add_to(&mut events);
        InvalidateEvent::ListMessages {
            account_id: "1".into(),
        }
        .add_to(&mut events);
        assert_eq!(
            events,
            vec![
                InvalidateEvent::Accounts,
                InvalidateEvent::ListMessages {
                    account_id: "1".into()
                },
                InvalidateEvent::ListMessages {
                    account_id: "2".into()
                },
            ]
        );
    }
}
