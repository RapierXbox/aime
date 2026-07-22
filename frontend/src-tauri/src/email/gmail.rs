use std::sync::Arc;
use std::vec;
use std::{
    collections::HashMap, env::temp_dir, f64::consts::E, future, ops::ControlFlow, str::FromStr,
};

use base64::prelude::*;
use base64::Engine;
use futures::{task, StreamExt};
use google_gmail1::api::MessagePartBody;
use google_gmail1::hyper::client;
use google_gmail1::{
    api::{Message, MessagePart},
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::{error, info};
use log::{trace, warn};
use tauri::State;
use tokio::{fs::File, io::AsyncWriteExt, stream};

use crate::email::repo::{GmailRepo, MessageContents, MissingField};
use crate::{
    email::{
        repo::{self, AccountConfig, AddLabelStatus, EmailAccount, HistoryID},
        EmailManager,
    },
    AppError, DbPool,
};

pub type GmailApiClient = google_gmail1::Gmail<HttpsConnector<HttpConnector>>;

pub mod auth;

pub struct GmailClient {
    client: GmailApiClient,
    account_id: i64,
    email_addr: String,
    repo: GmailRepo,
}

impl std::fmt::Debug for GmailClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GmailClient")
            .field("client", &"GmailClient { .. }")
            .field("account_id", &self.account_id)
            .field("email_addr", &self.email_addr)
            .field("repo", &self.repo)
            .finish()
    }
}

impl GmailClient {
    pub fn new(
        account_id: i64,
        db_pool: DbPool,
        email_addr: String,
        client: GmailApiClient,
    ) -> Self {
        Self {
            client,
            account_id,
            email_addr,
            repo: GmailRepo::new(db_pool, account_id),
        }
    }

    pub fn email_addr(&self) -> &str {
        &self.email_addr
    }
    pub fn gmail_client(&self) -> &GmailApiClient {
        &self.client
    }

    /// labels are used for sorting messages into inboxes as well as user defined labels
    /// this function refetches the user's available labels and (will) invalidate the frontend labels query
    async fn sync_labels(&self) -> Result<(), crate::AppError> {
        // list the labels from the gmail api
        let (_, res) = self
            .client
            .users()
            .labels_list("me")
            .doit()
            .await
            .map_err(|e| {
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
            // extract the required fields
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
                return Err(AppError::from(GmailError::MissingField(
                    MissingField::InLabel,
                )));
            };

            let status = self
                .repo
                .add_label(repo::Label {
                    id,
                    name,
                    message_list_visibility,
                    label_list_visibility,
                    type_,
                })
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

    pub async fn full_sync(&self) -> Result<(), crate::AppError> {
        // TODO: include spam/trash? decide: lazy sync inboxes?

        self.sync_labels().await?;
        self.fetch_and_store_message_skeletons().await?;
        self.backfill_messages().await?;

        info!(
            "full_sync: finished backfilling all messages for account_id={}",
            self.account_id
        );

        let latest_history_id = self.repo.get_latest_sync_cursor().await?;
        let config = self.repo.get_account_config().await?;

        let AccountConfig::Gmail { scopes, .. } = config else {
            unreachable!()
        };

        let new_config = AccountConfig::Gmail {
            history_id: HistoryID::LastSynced(latest_history_id),
            scopes,
        };

        info!(
            "full sync: updated account config account_id={}, new config={new_config:?}",
            self.account_id
        );

        self.repo.set_account_config(&new_config).await?;

        Ok(())
    }

    const N_FETCH_WORKERS: usize = 4;
    async fn backfill_messages(&self) -> Result<(), AppError> {
        let stream = self.repo.stream_message_skeletons();
        info!(
            "starting parallel backfill for account_id={}",
            self.account_id
        );

        // fetch the messages in parallel and load them into the db
        stream
            .map(|it| async { self.fetch_and_store_message(&it?.provider_msg_id).await })
            .buffer_unordered(Self::N_FETCH_WORKERS)
            .for_each(|res| async {
                if let Err(e) = res {
                    error!("failed to load message: {e:?}");
                }
            })
            .await;

        Ok(())
    }

    /// fetch, parse and store a single message
    async fn fetch_and_store_message(&self, msg_id: &str) -> Result<(), crate::AppError> {
        let (_, msg) = self
            .client
            .users()
            .messages_get("me", msg_id)
            .doit()
            .await?;

        let message = Self::parse_message(msg)?;

        self.repo.store_message(message).await
    }

    /// Synchronize and store the message skeletons from users.messages.list
    async fn fetch_and_store_message_skeletons(&self) -> Result<(), AppError> {
        // https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list
        // list the first 500 messages
        let (b, res) = self
            .client
            .users()
            .messages_list("me")
            .max_results(500)
            .doit()
            .await
            .map_err(|e| {
                error!("failed to list messages: {e:#?}");
                GmailApiError::from(e)
            })?;

        let Some(messages) = &res.messages else {
            error!("messages_list returned no messages field {b:#?}");
            return Err(crate::AppError::GmailResponseIncomplete);
        };

        self.repo.insert_skeleton_messages(&messages).await?;

        let mut next_page_token = res.next_page_token;
        // until no new page token is returned,
        while let Some(token) = next_page_token {
            info!("fetching messages page token={token}");

            // fetch the next 500 results
            let (b, res) = self
                .client
                .users()
                .messages_list("me")
                .max_results(500)
                .page_token(&token)
                .doit()
                .await
                .map_err(|e| {
                    error!("failed to list messages: {e:#?}");
                    GmailApiError::from(e)
                })?;

            let Some(messages) = &res.messages else {
                error!("messages_list returned no messages field {b:#?}");
                return Err(crate::AppError::GmailResponseIncomplete);
            };

            // and insert them into the table to be stored later
            self.repo.insert_skeleton_messages(&messages).await?;

            next_page_token = res.next_page_token;
        }

        trace!(
            "full_sync: finished listing all messages for account_id={}",
            self.account_id
        );

        Ok(())
    }

    fn parse_message(msg: Message) -> Result<repo::Message, AppError> {
        let Some(payload) = msg.payload.clone() else {
            return Err(GmailError::MissingField(repo::MissingField::Payload).into());
        };

        // parse headers
        let Some(headers) = payload.headers.clone() else {
            return Err(GmailError::MissingField(repo::MissingField::Headers).into());
        };

        let mut header_map: HashMap<String, String> =
            HashMap::from_iter(headers.into_iter().filter_map(|it| it.name.zip(it.value)));

        let Some(msg_id) = msg.id.clone() else {
            return Err(GmailError::MissingField(repo::MissingField::MsgId).into());
        };

        // TODO: implement tree-walking for this,
        // currently only supports one layer of container mesages
        let contents: Vec<repo::MessageContents> = match payload {
            // container MIME message part
            // assumes all children are leafs
            MessagePart {
                parts: Some(parts),
                headers,
                ..
            } => parts
                .into_iter()
                .map(|it| Self::parse_mime_leaf(it, msg_id.clone()))
                .filter_map(|it| match it {
                    Ok(v) => Some(v),
                    Err(e) => {
                        error!("skipping message part headers={headers:?} inside msg_id={msg_id} err={e:?}");
                        None
                    }
                })
                .collect::<Vec<MessageContents>>(),

            // this arm conflates valid, single container parts with parts we cannot parse yet
            // this currently skips deeply nested mime trees for example.
            _ => {
                let contents = Self::parse_mime_leaf(payload, msg_id.clone());
                match contents {
                    Err(e) => {
                        error!("skipped parsing content for msg_id={} due to {e:?}", msg_id);
                        vec![]
                    }
                    Ok(res) => vec![res],
                }
            }
        };

        let sync_cursor = msg.history_id.map(|id| id.to_string());
        let thread_id = msg.thread_id;

        let date_header = header_map.remove("Date");
        let from_addr = header_map.remove("From");
        let to_addrs = header_map.remove("To");
        let cc_addrs = header_map.remove("Cc");
        let subject = header_map.remove("Subject");

        let snippet = msg.snippet;

        Ok(repo::Message {
            contents,
            sync_cursor,
            thread_id,
            date_header,
            from_addr,
            to_addrs,
            cc_addrs,
            subject,
            snippet,
            label_ids: msg.label_ids.unwrap_or_default(),
            internal_date: msg.internal_date,
            provider_msg_id: msg.id,
            size_estimate: msg.size_estimate,
        })
    }

    /// If the MessagePart is a leaf part, parse into MessageContents
    /// filters accepted mime types and validates utf8
    fn parse_mime_leaf(
        part: MessagePart,
        provider_msg_id: String,
    ) -> Result<repo::MessageContents, AppError> {
        let MessagePart {
            body:
                Some(MessagePartBody {
                    data: Some(body_data),
                    ..
                }),
            mime_type: Some(mime_type),
            ..
        } = part
        else {
            return Err(AppError::GmailErr(GmailError::MessagePartNotLeaf));
        };

        if !matches!(mime_type.as_str(), "text/plain" | "text/html") {
            warn!("skipping inserting msg_part with mime_type {mime_type} for msg_id={provider_msg_id}");
            return Err(AppError::from(GmailError::UnsupportedMimeType));
        }

        // TODO: remove this copy, add lifetime to MessageContents
        let body = String::from_utf8(body_data).map_err(|e| {
            error!("decode error while inserting message part for msg_id={provider_msg_id}: {e:?}");
            AppError::InvalidEmailBody
        })?;

        Ok(repo::MessageContents {
            body,
            provider_msg_id,
            mime_type,
        })
    }
}

// TODO: extract into (own) Gmail struct that has Gmail::full_sync and contains all relevant state

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

    #[error("unsupported MIME content type")]
    UnsupportedMimeType,

    #[error("MIME Message part is not a leaf")]
    MessagePartNotLeaf,

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

    let tmp_gmail = GmailApiClient::new(client.clone(), tmp_auth.clone());
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

    let gm = GmailApiClient::new(client, auth);

    email_mng.account_map.lock().await.insert(
        id,
        Arc::new(GmailClient::new(id, db_pool.inner().clone(), email, gm)),
    );

    Ok(())
}
