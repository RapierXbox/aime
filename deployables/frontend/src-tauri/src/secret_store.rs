//! Get and set keyring passwords for this app
//! when  cfg(debug_assertions), the passwords are stored in a text file

#[cfg(debug_assertions)]
use std::{collections::HashMap, io::Write, path::PathBuf, sync::OnceLock};

#[cfg(debug_assertions)]
use log::error;
use log::info;
use tauri_plugin_keyring::KeyringExt;

use crate::{AppError, KEYRING_SERVICE};

/// The keyring key for storing a refresh token for a given gmail address
pub fn fmt_gmail_keyring_user(email_addr: &str) -> String {
    format!("gmail:refresh:{}", email_addr)
}

#[cfg(not(debug_assertions))]
/// Use one of the functions in this module to format the email address to the appropriate keyring key
pub fn set_password(
    app: &tauri::AppHandle,
    keyring_key: &str,
    refresh_token: &str,
) -> Result<(), crate::AppError> {
    app.keyring()
        .set_password(KEYRING_SERVICE, &keyring_key, refresh_token)
        .inspect_err(|e| info!("could not save {:?} to keyring: {:?}", keyring_key, e))
        .map_err(|_| crate::AppError::KeyringSaveError)
}

#[cfg(not(debug_assertions))]
/// Use one of the functions in this module to format the email address to the appropriate keyring key
pub fn get_password(
    app: &tauri::AppHandle,
    keyring_key: &str,
) -> Result<Option<String>, crate::AppError> {
    app.keyring()
        .get_password(KEYRING_SERVICE, &keyring_key)
        .map_err(|e| AppError::KeyringLoadError)
}

#[cfg(debug_assertions)]
static DEV_KEYRING_MAP_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Use one of the functions in this module to format the email address to the appropriate keyring key
#[cfg(debug_assertions)]
pub fn set_password(
    app: &tauri::AppHandle,
    keyring_key: &str,
    refresh_token: &str,
) -> Result<(), crate::AppError> {
    let path = DEV_KEYRING_MAP_PATH.get_or_init(|| create_dev_token_store_file(app));

    let str = std::fs::read_to_string(path).map_err(|e| {
        error!("failed to read dev keyring token store in {path:?} due to {e:?}");
        crate::AppError::KeyringLoadError
    })?;

    let mut map: HashMap<String, String> = serde_json::from_str(&str).map_err(|e| {
        error!("failed to parse dev keyring token store: {e:?}");
        AppError::KeyringLoadError
    })?;

    map.insert(keyring_key.to_string(), refresh_token.to_string());

    std::fs::write(
        path,
        serde_json::to_string(&map).map_err(|e| {
            error!("failed to serialize dev keyring token store: {e:?}");
            crate::AppError::KeyringSaveError
        })?,
    )
    .map_err(|e| {
        error!("failed to write dev kering token store {e:?}");
        crate::AppError::KeyringSaveError
    })?;

    info!("saved password for keyring_key={keyring_key} in mock key store");

    Ok(())
}

#[cfg(debug_assertions)]
/// Use one of the functions in this module to format the email address to the appropriate keyring key
pub fn get_password(
    app: &tauri::AppHandle,
    keyring_key: &str,
) -> Result<Option<String>, crate::AppError> {
    let path = DEV_KEYRING_MAP_PATH.get_or_init(|| create_dev_token_store_file(app));

    let str = std::fs::read_to_string(path).map_err(|e| {
        error!("failed to read dev keyring token store in {path:?} due to {e:?}");
        crate::AppError::KeyringLoadError
    })?;

    let map: HashMap<String, String> = serde_json::from_str(&str).map_err(|e| {
        error!("failed to parse dev keyring token store: {e:?}");
        AppError::KeyringLoadError
    })?;

    info!("read password for keyring_key={keyring_key} from mock key store");
    Ok(map.get(keyring_key).cloned())
}

#[cfg(debug_assertions)]
/// Creates the dev token store file in the user's data directory if it does not exist
/// Returns the path to the dev token store file
fn create_dev_token_store_file(app: &tauri::AppHandle) -> std::path::PathBuf {
    use tauri::Manager;

    let p = app
        .path()
        .app_data_dir()
        .unwrap()
        .join("dev_token_store.json");

    if !p.parent().unwrap().exists() {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        info!("creating data dir for dev token store {p:?}");
    }

    if !p.exists() {
        let mut f = std::fs::File::create(&p).unwrap();
        f.write(b"{}").unwrap();
        info!("creating dev token store file {p:?}");
    }

    p
}
