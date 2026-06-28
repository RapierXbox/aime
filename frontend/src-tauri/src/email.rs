use std::collections::HashMap;

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
use tauri::{async_runtime, App, Manager, State};
use tokio::sync::Mutex;

use crate::{
    email::{
        gmail::auth::Auth,
        repo::{add_label, AddLabelStatus},
    },
    DbPool,
};

pub mod gmail;
mod repo;

pub struct EmailManager {
    // Maps account_id to (email, Auth)
    // to use: construct a gmail client with the http_client
    // TODO: add struct, extract Auth into enum, maybe RwLock
    account_map: Mutex<HashMap<i64, (String, gmail::auth::Auth)>>,
    /// Hyper Client, cheap to Clone
    http_client: Client<HttpsConnector<HttpConnector>>,
    oauth_reqwest_client: reqwest::Client,
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
    let auth_clients = {
        let oauth_http_client = oauth_http_client.clone();

        accs.into_iter().map(move |acc| {
            let email = acc.email.clone();
            let account_id = acc.id;
            (
                email,
                account_id,
                Auth::reinstantiate(&app_handle, acc.clone(), oauth_http_client.clone()),
            )
        })
    };

    let mut email_account_map = HashMap::new();

    // populate the email account map from the db
    for (email, account_id, auth_client) in auth_clients {
        let auth_client = auth_client?;
        email_account_map.insert(account_id, (email, auth_client));
    }

    info!("{email_account_map:?}");

    app.manage(EmailManager {
        account_map: Mutex::new(email_account_map),
        http_client: gmail_http_client,
        oauth_reqwest_client: oauth_http_client.clone(),
    });

    Ok(())
}

// Use for listing email accounts in the UI
#[derive(Serialize, Debug, Clone)]
pub struct ListEmailEntry {
    id: i64,
    name: String,
}

#[tauri::command]
/// list all email accounts registered with the application
pub async fn email_list_accounts(
    email: State<'_, EmailManager>,
) -> Result<Vec<ListEmailEntry>, crate::Error> {
    let lock = email.account_map.lock().await;
    Ok(lock
        .iter()
        .map(|(&id, (name, _))| ListEmailEntry {
            id,
            name: name.clone(),
        })
        .collect())
}

#[tauri::command]
pub async fn dev_do_onboard_sync(
    account_id: i64,
    email_mng: State<'_, EmailManager>,
    db_pool: State<'_, DbPool>,
) -> Result<(), crate::Error> {
    info!("do_onboard_sync for {account_id}");

    onboard_sync(account_id, email_mng, db_pool).await
}

pub async fn onboard_sync(
    account_id: i64,
    email_mng: State<'_, EmailManager>,
    db_pool: State<'_, DbPool>,
) -> Result<(), crate::Error> {
    // get the account and instantiate the gmail client
    let lock = email_mng.account_map.lock().await;
    let (_email, auth_ref) = lock.get(&account_id).unwrap();

    let auth = auth_ref.clone();
    let client = email_mng.http_client.clone();

    let gmail = Gmail::new(client, auth);

    // first, list the labels and store them in the db
    let (_, res) = gmail.users().labels_list("me").doit().await?;

    let Some(labels) = res.labels else {
        error!("gmail sync no labels returned");
        return Err(crate::Error::GmailMissingLabels);
    };

    info!("Got Labels: {labels:#?}");

    // wether any new label was added
    let mut label_status = AddLabelStatus::AlreadyExists;

    // From docs:
    // List of labels. Note that each label resource only contains an id, name, messageListVisibility, labelListVisibility, and type
    for label in labels {
        let Label {
            id: Some(id),
            name: Some(name),
            message_list_visibility,
            label_list_visibility,
            type_: Some(type_),
            ..
        } = label
        else {
            error!("label missing required fields {:?}", label);
            unreachable!();
        };

        let status = add_label(
            &db_pool,
            repo::Label {
                id,
                name,
                message_list_visibility,
                label_list_visibility,
                type_,
            },
        )
        .await?;

        if matches!(status, AddLabelStatus::Inserted) {
            label_status = AddLabelStatus::Inserted;
        }
    }

    // TODO: invalidate frontend labels
    if matches!(label_status, AddLabelStatus::Inserted) {
        info!("New Label was added");
    }

    Ok(())
}
