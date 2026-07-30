//! # Email Management
//!
//! ## Inboxes
//! The default inboxes that should be displayed to the user are INBOX, SENT, DRAFT, STARRED, TRASH, SPAM
//! Use with `enum MailBox`
//! They are saved as labels and attached via message_has_label

use std::{collections::HashMap, sync::Arc};

use google_gmail1::{
    api::Label,
    common::Client,
    hyper_rustls::{self, HttpsConnector},
    hyper_util::{self, client::legacy::connect::HttpConnector},
    Gmail,
};
use log::{error, info};
use oauth2::reqwest;
use serde::{Deserialize, Serialize};
use specta::Type;
use tauri::{async_runtime, ipc::Channel, window::ProgressBarState, App, Manager, State};
use tokio::sync::Mutex;

use crate::{
    email::{
        self,
        gmail::{auth::Auth, GmailApiClient},
    },
    AppError, DbPool, Progress,
};

pub mod gmail;
pub mod repo;

#[derive(Debug, Clone, Serialize, Type, sqlx::Type)]
#[serde(rename_all = "UPPERCASE")]
#[sqlx(type_name = "varchar", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MailBox {
    Inbox,
    Sent,
    Draft,
    Starred,
    Trash,
    Spam,
}

#[derive(Clone, Debug)]
pub struct EmailManager {
    // Maps account_id to (email_address, Auth)
    // to use: construct a gmail client with the http_client
    // TODO: wrap in provider-agnostic enum, maybe RwLock
    account_map: Arc<Mutex<HashMap<i64, Arc<email::gmail::GmailClient>>>>,
    /// Hyper Client, cheap to Clone
    http_client: Client<HttpsConnector<HttpConnector>>,
    /// only used for the auth requests
    oauth_reqwest_client: reqwest::Client,
}

impl EmailManager {
    pub async fn get_client(&self, account_id: i64) -> Option<Arc<gmail::GmailClient>> {
        let lock = self.account_map.lock().await;
        lock.get(&account_id).cloned()
    }
}

/// Run on setup
/// Instantiates: State<'_, EMailManager>
///
/// Loads email accounts on startup from the database
pub fn setup(app: &mut App, db: &DbPool) -> Result<(), Box<dyn std::error::Error>> {
    // google_gmail1 uses a hyper client
    let gmail_http_client =
        hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).build(
            hyper_rustls::HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_or_http()
                .enable_http2()
                .build(),
        );

    // while oauth2 uses a reqwest client,
    // so we need to deal with 2 clients right now.
    // TODO: check if you can use hyper for both
    let oauth_http_client = reqwest::ClientBuilder::new()
        // Following redirects opens the client up to SSRF vulnerabilities.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("Client should build");

    let app_handle = app.handle();
    let accs = async_runtime::block_on(repo::get_accounts(&db))?;

    // populate the email account map from the db
    let auth_clients = {
        let oauth_http_client = oauth_http_client.clone();

        accs.into_iter()
            .map(move |acc| {
                let email = acc.email.clone();
                let account_id = acc.id;
                (
                    email,
                    account_id,
                    Auth::new(&app_handle, acc.clone(), oauth_http_client.clone()),
                )
            })
            .filter_map(|(email, account_id, auth_client)| match auth_client {
                Ok(auth) => Some((
                    account_id,
                    Arc::new(gmail::GmailClient::new(
                        account_id,
                        db.clone(),
                        email,
                        GmailApiClient::new(gmail_http_client.clone(), auth),
                    )),
                )),
                Err(_) => None,
            })
    };

    let mut email_account_map = HashMap::from_iter(auth_clients);

    info!("{email_account_map:?}");

    app.manage(EmailManager {
        account_map: Arc::new(Mutex::new(email_account_map)),
        http_client: gmail_http_client,
        oauth_reqwest_client: oauth_http_client.clone(),
    });

    Ok(())
}

// Use for listing email accounts in the UI
#[derive(Serialize, Debug, Clone, specta::Type)]
pub struct ListEmailEntry {
    id: String,
    name: String,
}

#[tauri::command]
#[specta::specta]
/// list all email accounts registered with the application
pub async fn email_list_accounts(
    email: State<'_, EmailManager>,
) -> Result<Vec<ListEmailEntry>, crate::AppError> {
    let lock = email.account_map.lock().await;
    Ok(lock
        .iter()
        .map(|(&id, it)| ListEmailEntry {
            id: id.to_string(),
            name: it.email_addr().to_owned(),
        })
        .collect())
}

#[tauri::command]
#[specta::specta]
/// Syncs the email account with the given ID
///
/// ### Arguments
/// - `account_id`: The ID of the account to sync, a u64 serialized as a String
///
pub async fn dev_email_full_sync(
    account_id: String,
    email_mng: State<'_, EmailManager>,
    update_channel: Channel<Progress>,
    db_pool: State<'_, DbPool>,
) -> Result<(), crate::AppError> {
    info!("do_onboard_sync for {account_id}");

    let account_id = account_id
        .parse::<i64>()
        .map_err(|_| crate::AppError::ParseAccountID)?;

    let client = email_mng
        .inner()
        .get_client(account_id)
        .await
        .ok_or(AppError::AccountNotFound)?;

    client.full_sync(update_channel).await
}

#[tauri::command]
#[specta::specta]
pub async fn email_sync(
    account_id: String,
    update_channel: Channel<Progress>,
    email_mng: State<'_, EmailManager>,
) -> Result<(), crate::AppError> {
    let account_id = account_id
        .parse::<i64>()
        .map_err(|_| crate::AppError::ParseAccountID)?;

    let client = email_mng
        .inner()
        .get_client(account_id)
        .await
        .ok_or(AppError::AccountNotFound)?;

    let res = client.sync(update_channel).await;
    info!("email_sync res = {res:?}");
    res
}

/// return a list of message stubs
#[tauri::command]
#[specta::specta]
pub async fn list_messages(
    account_id: String,
    page_index: u32,
    mailbox: MailBox,
    email_mng: State<'_, EmailManager>,
) -> Result<Vec<repo::Message>, AppError> {
    let account_id = account_id
        .parse::<i64>()
        .map_err(|_| crate::AppError::ParseAccountID)?;

    let client = email_mng
        .inner()
        .get_client(account_id)
        .await
        .ok_or(AppError::AccountNotFound)?;

    client.list_messages(mailbox, page_index).await
}
