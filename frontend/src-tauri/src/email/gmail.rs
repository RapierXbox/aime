use std::{
    collections::HashMap, env::temp_dir, f64::consts::E, future, ops::ControlFlow, str::FromStr,
};

use base64::prelude::*;
use base64::Engine;
use futures::{task, StreamExt};
use google_gmail1::api::MessagePartBody;
use google_gmail1::{
    api::{ListThreadsResponse, Message, MessagePart},
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::{error, info};
use tauri::{utils::mime_type, State};
use tokio::{fs::File, io::AsyncWriteExt, stream};

use crate::{
    email::{
        repo::{self, AccountConfig, AddLabelStatus, EmailAccount, HistoryID},
        EmailManager,
    },
    AppError::{self, GmailApiErr},
    DbPool,
};

pub type Gmail = google_gmail1::Gmail<HttpsConnector<HttpConnector<GaiResolver>>>;

pub mod auth;

#[derive(thiserror::Error, Debug, serde::Serialize, specta::Type)]
pub enum GmailApiError {
    #[error("HTTP error")]
    HttpError,
    #[error("Upload size limit exceeded: resource {resource_size}, max {max_size}")]
    // since specta::Type doesnt allow u64, which would be correct,
    // we use f64 and accept the precision loss
    UploadSizeLimitExceeded { resource_size: f64, max_size: f64 },
    #[error("Bad request")]
    BadRequest,
    #[error("Missing API key")]
    MissingAPIKey,
    #[error("Missing token")]
    MissingToken,
    #[error("Operation cancelled")]
    Cancelled,
    #[error("Field clash")]
    FieldClash,
    #[error("JSON decode error")]
    JsonDecodeError,
    #[error("Request failure: {0}")]
    // contains http status code
    Failure(u16),
    #[error("IO error")]
    Io,
}

impl From<google_gmail1::Error> for AppError {
    fn from(value: google_gmail1::Error) -> Self {
        GmailApiError::from(value).into()
    }
}

impl From<google_gmail1::Error> for GmailApiError {
    fn from(e: google_gmail1::Error) -> Self {
        match e {
            google_gmail1::Error::HttpError(_) => Self::HttpError,
            google_gmail1::Error::UploadSizeLimitExceeded(resource_size, max_size) => {
                Self::UploadSizeLimitExceeded {
                    resource_size: resource_size as f64,
                    max_size: max_size as f64,
                }
            }
            google_gmail1::Error::BadRequest(_) => Self::BadRequest,
            google_gmail1::Error::MissingAPIKey => Self::MissingAPIKey,
            google_gmail1::Error::MissingToken(_) => Self::MissingToken,
            google_gmail1::Error::Cancelled => Self::Cancelled,
            google_gmail1::Error::FieldClash(_) => Self::FieldClash,
            google_gmail1::Error::JsonDecodeError(_, _) => Self::JsonDecodeError,
            google_gmail1::Error::Failure(r) => Self::Failure(r.status().as_u16()),
            google_gmail1::Error::Io(_) => Self::Io,
        }
    }
}

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
    #[error("while responding to oauth2 redirect")]
    OauthRedirect,

    #[error("while responding to oauth2 redirect")]
    OauthHttpResp,

    #[error("while establishing tcp listener for oauth2 redirect")]
    OauthTcpListen,

    #[error("message is not a skeleton message")]
    MessageNotSkeleton,

    #[error("unsupported MIME-Version header")]
    UnsupportedMimeVer,

    #[error("Missing field {0:?} in response")]
    MissingField(repo::MissingField),
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
            GmailApiError::from(e)
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

    sync_labels(&gmail, db_pool.inner()).await?;

    // TODO: include spam/trash? decide: lazy sync inboxes?

    // https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list
    // list the first 500 messages
    let (b, res) = gmail
        .users()
        .messages_list("me")
        .max_results(500)
        .doit()
        .await
        .map_err(|e| {
            error!("failed to list messages: {e:#?}");
            GmailApiError::from(e)
        })?;

    let Some(messages) = res.messages else {
        error!("messages_list returned no messages field {b:#?}");
        return Err(crate::AppError::GmailResponseIncomplete);
    };

    insert_skeleton_msgs(db_pool.inner(), account_id, &messages).await?;
    tauri::async_runtime::spawn(load_emails_task(
        db_pool.inner().clone(),
        account_id,
        email_mng.inner().clone(),
    ));

    Ok(())
}

async fn insert_skeleton_msgs(
    db_pool: &DbPool,
    account_id: i64,
    msgs: &[Message],
) -> Result<(), crate::AppError> {
    let mut tx = db_pool.begin().await?;

    for msg in msgs {
        match msg {
            Message {
                id: Some(msg_id),
                thread_id: Some(thread_id),
                ..
            } => {
                let _ = sqlx::query!(
                    // TODO update labels?
                "INSERT INTO messages (account_id, provider_msg_id, thread_id) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                account_id,
                msg_id,
                thread_id,
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| {
                    error!("failed to insert skeleton msg: {e:?}");
                    AppError::Sqlx(e.into())
                })
                .map(|_| ());
            }

            _ => {
                error!("skipped inserting skeleton_msg into db: {msg:?}");
            }
        };
    }

    tx.commit().await?;

    Ok(())
}

const N_FETCH_WORKERS: usize = 4;

async fn load_emails_task(
    db_pool: DbPool,
    account_id: i64,
    email_mng: EmailManager,
) -> Result<(), crate::AppError> {
    // TODO loop after mime parsing
    let res = sqlx::query!(
        "SELECT (provider_msg_id) FROM messages 
            WHERE internal_date IS NULL 
                AND account_id = ? 
            ORDER BY sync_cursor DESC 
            LIMIT 100",
        account_id
    )
    .fetch(&db_pool);

    let gmail = email_mng.get_gmail_client(account_id).await?;

    // fetch the messages in parallel and load them into the db
    res.map(|it| async { fetch_message(account_id, &it?.provider_msg_id, &gmail, &db_pool).await })
        .buffer_unordered(N_FETCH_WORKERS)
        .for_each(|res| async {
            if let Err(e) = res {
                error!("failed to load message: {e:?}");
            }
        })
        .await;

    //load_emails(&db_pool, account_id).await;

    Ok(())
}

async fn fetch_message(
    account_id: i64,
    msg_id: &str,
    gmail: &Gmail,
    db_pool: &DbPool,
) -> Result<(), crate::AppError> {
    info!("email: {:?}", msg_id);
    let (_, msg) = gmail.users().messages_get("me", msg_id).doit().await?;

    insert_msg(account_id, msg, db_pool).await
}

use serde_json::Value as JsonValue; // For the JSON array column

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MessagesRow {
    pub account_id: i64,
    pub provider_msg_id: String,
    pub thread_id: Option<String>,
    pub sync_cursor: Option<String>,
    /// Stored as a JSON string/text array in SQLite.
    /// If you prefer a strongly typed struct, replace `JsonValue` with `sqlx::types::Json<YourStruct>`.
    pub label_ids: Option<String>,

    pub internal_date: Option<i64>,
    pub size_estimate: Option<i64>,
    pub date_header: Option<String>,

    pub from_addr: Option<String>,
    pub to_addrs: Option<String>,
    pub cc_addrs: Option<String>,

    pub in_reply_to: Option<String>,
    pub msg_references: Option<String>,

    pub subject: Option<String>,
    pub snippet: Option<String>,
}

// Gmail API messages are normalized to utf-8
async fn insert_msg(
    account_id: i64,
    msg: Message,
    db_pool: &DbPool,
) -> Result<(), crate::AppError> {
    let Some(payload) = msg.payload else {
        return Err(GmailError::MissingField(repo::MissingField::Payload).into());
    };

    // parse headers
    let Some(headers) = payload.headers else {
        return Err(GmailError::MissingField(repo::MissingField::Headers).into());
    };

    let header_map: HashMap<String, String> =
        HashMap::from_iter(headers.into_iter().filter_map(|it| it.name.zip(it.value)));

    // TODO
    let msg_id = msg.id.unwrap();

    match payload {
        MessagePart {
            body:
                Some(MessagePartBody {
                    data: Some(body_data),
                    ..
                }),
            mime_type: Some(mime_type),
            ..
        } => {
            let body_decoded = BASE64_URL_SAFE.decode(body_data).map_err(|e| {
                error!("decode error: {e:?}");
                AppError::InvalidEmailBody
            })?;

            sqlx::query!(
                "INSERT INTO message_contents 
                (account_id, provider_msg_id, mime_type, body)
                VALUES (?, ?, ?, ?)",
                account_id,
                msg_id,
                mime_type,
                body_decoded
            )
            .execute(db_pool)
            .await
            .inspect_err(|e| {
                error!("failed to insert message content: {e:?}");
            })?;
        }

        // container MIME message part
        MessagePart {
            parts: Some(parts), ..
        } => {
            info!("skipping container message {msg_id}");
            return Ok(());
        }

        MessagePart {
            body,
            filename,
            mime_type,
            part_id,
            parts,
            ..
        } => {
            info!("skipping message {msg_id} with no body: fname={filename:?}, mime_type={mime_type:?}");
            return Ok(());
        }
    }

    for label in msg.label_ids.as_ref().unwrap_or(&vec![]) {
        sqlx::query!(
            "INSERT INTO message_has_label (account_id, provider_msg_id, label_id) VALUES (?, ?, ?)",
            account_id,
            msg_id,
            label
        )
        .execute(db_pool)
        .await?;
    }

    let history_id = msg.history_id.map(|id| id.to_string());
    let date_header = header_map.get("Date");
    let from_addr = header_map.get("From");
    let to_addrs = header_map.get("To");
    let cc_addrs = header_map.get("Cc");
    let subject = header_map.get("Subject");

    sqlx::query!(
        "UPDATE messages SET

            thread_id = COALESCE(?, thread_id),
            sync_cursor = ?,
            internal_date = ?,
            size_estimate = ?,

            date_header = ?,
            from_addr = ?,
            to_addrs = ?,
            cc_addrs = ?,
            in_reply_to = ?,
            msg_references = ?,
            subject = ?,
            snippet = ?
        WHERE account_id = ? AND provider_msg_id = ?",
        msg.thread_id,
        history_id,
        msg.internal_date,
        msg.size_estimate,
        date_header,
        from_addr,
        to_addrs,
        cc_addrs,
        None::<String>,
        None::<String>,
        subject,
        msg.snippet,
        account_id,
        msg_id
    )
    .execute(db_pool)
    .await?;

    Ok(())
}

mod mime {
    use google_gmail1::api::MessagePart;

    pub fn is_container(part: &MessagePart) -> bool {
        part.parts.as_ref().is_some_and(|p| !p.is_empty())
    }

    pub fn is_leaf_text(part: &MessagePart) -> bool {
        matches!(
            part.mime_type.as_deref(),
            Some("text/plain") | Some("text/html")
        )
    }

    pub async fn insert_part_body(part: &MessagePart) -> Result<(), crate::AppError> {
        todo!()
    }
}

/// Fetches and syncs the labels for the given Gmail account.
///
/// Labels represent Mailboxes like INBOX, SPAM, etc.
/// or user-defined labels. (differentiated by the `type_` field)
async fn sync_labels(
    gmail: &google_gmail1::Gmail<HttpsConnector<HttpConnector>>,
    db_pool: &DbPool,
) -> Result<(), crate::AppError> {
    let (_, res) = gmail.users().labels_list("me").doit().await.map_err(|e| {
        error!("failed to list labels: {e:?}");
        GmailApiError::from(e)
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
