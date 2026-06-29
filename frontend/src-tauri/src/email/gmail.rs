use google_gmail1::{
    api::ListThreadsResponse,
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::{error, info};
use tauri::State;

use crate::{
    email::{
        gmail,
        repo::{self, add_label, AccountConfig, AddLabelStatus, EmailAccount, HistoryID},
        EmailManager,
    },
    DbPool,
};

pub type Gmail = google_gmail1::Gmail<HttpsConnector<HttpConnector<GaiResolver>>>;

pub mod auth;

#[derive(thiserror::Error, Debug, serde::Serialize, specta::Type)]
pub enum GmailError {
    // Auth Errors:
    #[error("while parsing auth url")]
    AuthUrlParse,
    #[error("while parsing token url")]
    TokenUrlParse,
    #[error("while parsing redirect url")]
    RedirectUrlParse,

    // Endpoint Errors:
    #[error("while listing labels")]
    ListLabels,

    #[error("while getting message")]
    GetMessage,

    #[error("while listing messages")]
    ListMessages,

    #[error("while getting profile")]
    GetProfile,

    #[error("while responding to oauth2 redirect")]
    OauthRedirect,

    #[error("while responding to oauth2 redirect")]
    OauthHttpResp,

    #[error("while establishing tcp listener for oauth2 redirect")]
    OauthTcpListen,
}

#[tauri::command]
#[specta::specta]
/// register a new gmail account with the oauth2 onboarding flow
pub async fn register_gmail_account(
    app: tauri::AppHandle,
    db_pool: State<'_, DbPool>,
    email_mng: State<'_, EmailManager>,
) -> Result<(), crate::AppError> {
    let client = email_mng.http_client.clone();

    let tmp_auth = auth::TempAuth::do_oauth2_flow(email_mng.oauth_reqwest_client.clone()).await?;

    let tmp_gmail = Gmail::new(client, tmp_auth.clone());
    let (_, profile) = tmp_gmail
        .users()
        .get_profile("me")
        .doit()
        .await
        .map_err(|e| {
            error!("failed to get profile {e:#?}");
            GmailError::GetProfile
        })?;

    // TODO: use Arc<str> for emails since they are cloned frequently
    // TODO: error checks
    let email = profile.email_address.clone().unwrap();

    tmp_auth.save_to_keyring(&app, &email)?;

    let id = repo::add_account(
        &db_pool,
        EmailAccount {
            id: -1,
            email: email.clone(),
            account_name: profile.email_address.clone().unwrap(),
            config: AccountConfig::Gmail {
                history_id: HistoryID::KnownStale,
                scopes: tmp_auth.scopes().to_vec(),
            },
        },
    )
    .await?;

    let auth = tmp_auth.promote(profile);
    // the gmail account with the authenticated user

    email_mng.account_map.lock().await.insert(id, (email, auth));

    Ok(())
}

// perform a full sync up to history_id_target
pub async fn full_sync(
    account_id: i64,
    email_mng: State<'_, EmailManager>,
    db_pool: State<'_, DbPool>,
) -> Result<(), crate::AppError> {
    // get the account and instantiate the gmail client
    let lock = email_mng.account_map.lock().await;
    let (_email, auth_ref) = lock.get(&account_id).unwrap();

    let auth = auth_ref.clone();
    let client = email_mng.http_client.clone();

    let gmail = Gmail::new(client, auth);

    sync_labels(&gmail, db_pool).await?;

    // TODO: include spam/trash? decide: lazy sync inboxes?

    // https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.threads/list
    let (b, res) = gmail
        .users()
        .messages_list("me")
        .max_results(500)
        .doit()
        .await
        .map_err(|e| {
            error!("failed to list messages: {e:#?}");
            GmailError::ListMessages
        })?;

    let Some(ref messages) = res.messages else {
        error!("failed to get messages {b:#?}");
        todo!();
    };

    info!("received {res:#?}");

    if let Some(first) = messages.first() {
        info!("first message: {first:#?}");
        let (_, mut res) = gmail
            .users()
            .messages_get("me", first.id.as_ref().unwrap())
            .format("full")
            .doit()
            .await
            .map_err(|e| {
                error!("failed to GET users.messages {e:#?}");
                GmailError::GetMessage
            })?;

        res.payload = None;
        info!("first message details: {res:#?}");
    }

    Ok(())
}

/// Fetches and syncs the labels for the given Gmail account.
///
/// Labels represent Mailboxes like INBOX, SPAM, etc.
/// or user-defined labels. (differentiated by the `type_` field)
async fn sync_labels(
    gmail: &google_gmail1::Gmail<HttpsConnector<HttpConnector>>,
    db_pool: State<'_, sqlx::Pool<sqlx::Sqlite>>,
) -> Result<(), crate::AppError> {
    let (_, res) = gmail.users().labels_list("me").doit().await.map_err(|e| {
        error!("failed to list labels: {e:?}");
        GmailError::ListLabels
    })?;

    let Some(labels) = res.labels else {
        error!("gmail sync no labels returned");
        return Err(crate::AppError::GmailMissingLabels);
    };

    info!("Got Labels: {labels:#?}");
    let mut label_status = AddLabelStatus::AlreadyExists;
    for label in labels {
        let google_gmail1::api::Label {
            id: Some(id),
            name: Some(name),
            message_list_visibility,
            label_list_visibility,
            type_: Some(type_),
            ..
        } = label
        else {
            error!("label missing required fields: {:?}", label);
            unreachable!();
        };

        let status = repo::add_label(
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

    if matches!(label_status, AddLabelStatus::Inserted) {
        info!("New Label was added");
        // TODO: invalidate frontend labels
    }

    Ok(())
}
