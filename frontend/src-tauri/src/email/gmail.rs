use std::sync::Arc;
use std::vec;
use std::{
    collections::HashMap, env::temp_dir, f64::consts::E, future, ops::ControlFlow, str::FromStr,
};

use base64::prelude::*;
use base64::Engine;
use futures::{task, StreamExt};
use google_gmail1::api::{History, ListHistoryResponse, MessagePartBody};
use google_gmail1::hyper::{self, client};
use google_gmail1::{
    api::MessagePart,
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::{error, info};
use log::{trace, warn};
use tauri::ipc::Channel;
use tauri::State;
use tokio::{fs::File, io::AsyncWriteExt, stream};

use crate::email::gmail::repo::{GmailRepo, ListMessages, MessageStream};
use crate::email::repo::{Label, Message, MessageSkeleton};
use crate::email::MailBox;
use crate::{email, Progress, ProgressReporter};
use crate::{
    email::{
        repo::{
            AccountConfig, AddLabelStatus, EmailAccount, HistoryID, MessageContents, MissingField,
        },
        EmailManager,
    },
    AppError, DbPool,
};

pub type GmailApiClient = google_gmail1::Gmail<HttpsConnector<HttpConnector>>;

pub mod auth;
pub mod repo;

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
    const N_FETCH_WORKERS: usize = 4;

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

    /// Synchronize this GmailClient with the Gmail API,
    /// follows [https://developers.google.com/workspace/gmail/api/guides/sync]
    ///
    /// Either performs a partial sync, or if not possible, a full sync.
    pub async fn sync(&self, update_channel: Channel<crate::Progress>) -> Result<(), AppError> {
        update_channel.report(Progress::Update {
            completed: 0,
            out_of: Some(3),
        });

        self.sync_labels().await?;

        update_channel.report(Progress::Update {
            completed: 1,
            out_of: Some(3),
        });

        info!("synced labels");

        let AccountConfig::Gmail { history_id, .. } = self.repo.get_account_config().await?;
        info!("got account config! history_id: {:?}", history_id);

        match history_id {
            HistoryID::LastSynced(to) => {
                info!("performing partial sync");

                let start_history_id = to.parse().map_err(|_| AppError::ParseAccountID)?;
                let res = self
                    .partial_sync(start_history_id, update_channel.clone())
                    .await;

                match res {
                    Ok(_) => {
                        info!("partial sync succeeded");
                        res
                    }

                    Err(e) => {
                        // if the partial sync fails, fall back to full sync
                        error!("partial sync failed: {e:?}, trying full sync");
                        // self.full_sync(update_channel.clone()).await
                        todo!()
                    }
                }
            }
            HistoryID::KnownStale => {
                info!("history_id is known stale, performing full sync");
                self.full_sync(update_channel.clone()).await
            }
        }
    }

    async fn partial_sync(
        &self,
        start_history_id: u64,
        updates: Channel<Progress>,
    ) -> Result<(), crate::AppError> {
        // first invalidate the sync cursor to avoid failed syncs messing up data
        self.repo
            .set_account_config_sync_cursor(HistoryID::KnownStale)
            .await?;

        let res = self
            .client
            .users()
            .history_list("me")
            .start_history_id(start_history_id)
            .doit()
            .await;

        updates.report(Progress::Update {
            completed: 2,
            out_of: Some(4),
        });

        match res {
            Err(e) => match e {
                // on 404, we need to perform a full_sync
                google_gmail1::Error::Failure(res)
                    if res.status() == hyper::StatusCode::NOT_FOUND =>
                {
                    warn!("got a 404 {res:?} on users.history.list. Performing full sync");
                    return Err(AppError::GmailResponseIncomplete);
                }
                _ => return Err(e.into()),
            },

            // no updates since the last sync
            Ok((
                _,
                ListHistoryResponse {
                    history: None,
                    history_id: Some(history_id_new),
                    ..
                },
            )) => {
                updates.report(Progress::Update {
                    completed: 3,
                    out_of: Some(3),
                });

                info!("got empty history list, completing!");
                self.repo
                    .set_account_config_sync_cursor(HistoryID::LastSynced(
                        history_id_new.to_string(),
                    ))
                    .await?;
                return Ok(());
            }

            // got some history, apply it
            Ok((
                _,
                ListHistoryResponse {
                    history: Some(history),
                    history_id: Some(history_id_new),
                    next_page_token,
                    ..
                },
            )) => {
                self.apply_partial_sync_history(
                    start_history_id,
                    updates.clone(),
                    history,
                    history_id_new,
                    next_page_token,
                )
                .await?;
            }

            Ok((d, r)) => {
                error!("got messages.list response, didnt match correctly! r={r:#?}");
                return Err(crate::AppError::GmailResponseIncomplete);
            }
        }

        // the added messages need to be loaded
        self.backfill_messages(self.repo.stream_message_skeletons().await?, updates)
            .await?;

        Ok(())
    }

    async fn apply_partial_sync_history(
        &self,
        start_history_id: u64,
        updates: Channel<Progress>,
        history: Vec<History>,
        history_id_new: u64,
        next_page_token: Option<String>,
    ) -> Result<(), AppError> {
        // update the history we already have
        self.repo.apply_history(history, updates.clone()).await?;
        // next up, check if there are more pages to fetch

        let mut running_page_token = next_page_token;
        while let Some(ref token) = running_page_token {
            info!(
                "incr sync res_history_id={history_id_new}, target={start_history_id} page 1 done"
            );

            let (_, res) = self
                .client
                .users()
                .history_list("me")
                .page_token(token)
                // use the history_id from the request response
                .start_history_id(start_history_id)
                .doit()
                .await?;

            match res {
                ListHistoryResponse {
                    history: Some(paged_history),
                    next_page_token,
                    ..
                } => {
                    self.repo
                        .apply_history(paged_history, updates.clone())
                        .await?;
                    running_page_token = next_page_token;
                }

                ListHistoryResponse {
                    history: None,
                    history_id: Some(history_id_finished),
                    ..
                } => {
                    self.backfill_messages(
                        self.repo.stream_message_skeletons().await?,
                        updates.clone(),
                    )
                    .await?;

                    self.repo
                        .set_account_config_sync_cursor(email::repo::HistoryID::LastSynced(
                            history_id_finished.to_string(),
                        ))
                        .await?;

                    return Ok(());
                }

                _ => return Err(crate::AppError::GmailResponseIncomplete),
            }
        }

        info!("finished partial sync to {history_id_new}");
        self.repo
            .set_account_config_sync_cursor(email::repo::HistoryID::LastSynced(
                history_id_new.to_string(),
            ))
            .await?;

        Ok(())
    }

    // https://developers.google.com/workspace/gmail/api/guides/sync#full-sync
    // todo: return u64
    // TODO: this misses deleted messages since the fetch_and_store_skeletons doesnt delete records that werent touched
    pub async fn full_sync(&self, updates: Channel<Progress>) -> Result<(), crate::AppError> {
        // TODO: include spam/trash? decide: lazy sync inboxes?

        self.fetch_and_store_message_skeletons(updates.clone())
            .await?;

        // refetch all messages
        self.backfill_messages(self.repo.stream_all_messages().await?, updates.clone())
            .await?;

        info!(
            "full_sync: finished backfilling all messages for account_id={}",
            self.account_id
        );
        // todo: should be the the first message in the messages.list response
        let latest_history_id = self.repo.get_latest_sync_cursor().await?;
        let config = self.repo.get_account_config().await?;

        let AccountConfig::Gmail { scopes, .. } = config;

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
                .add_label(Label {
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

    async fn backfill_messages(
        &self,
        msgs: MessageStream<impl futures::Stream<Item = Result<MessageSkeleton, AppError>>>,
        updates: Channel<Progress>,
    ) -> Result<(), AppError> {
        let MessageStream { stream, total } = msgs;

        info!(
            "starting backfill for account_id={} n_messages={total}",
            self.account_id
        );

        let completed = std::sync::atomic::AtomicU64::new(0);

        // fetch the messages in parallel and load them into the db
        stream
            .map(|it| async {
                let s = match it {
                    Ok(s) => s,
                    Err(e) => return error!("failed to read message skeleton: {e:?}"),
                };

                // gather message id for better logging
                let id = s.provider_msg_id.clone();

                if let Err(e) = self.fetch_and_store_skeleton(s).await {
                    error!("failed to load message id={id}: {e:?}");
                }
            })
            .buffer_unordered(Self::N_FETCH_WORKERS)
            .for_each(|()| {
                let n = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                updates.report(Progress::Update {
                    completed: n,
                    out_of: Some(total.max(n)),
                });

                async move {}
            })
            .await;

        Ok(())
    }

    /// fetch, parse and store a single message, backfilling a message skeleton
    /// this may also update the labelIds of a previously cached message
    async fn fetch_and_store_skeleton(&self, s: MessageSkeleton) -> Result<(), crate::AppError> {
        let (_, msg) = self
            .client
            .users()
            .messages_get("me", &s.provider_msg_id)
            .format(if s.previously_cached {
                "minimal"
            } else {
                "full"
            })
            .doit()
            .await?;

        let message = Self::parse_message(msg)?;

        self.repo.backfill_message(message).await
    }

    /// Synchronize and store the message skeletons from users.messages.list
    async fn fetch_and_store_message_skeletons(
        &self,
        progress: Channel<Progress>,
    ) -> Result<(), AppError> {
        // https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list
        // list the first 500 messages
        let (b, res) = self
            .client
            .users()
            .messages_list("me")
            .include_spam_trash(true)
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
        let mut id = 1;
        while let Some(token) = next_page_token {
            id += 1;
            progress.report(Progress::Update {
                completed: id,
                out_of: Some(10.max(id + 2)),
            });

            info!("fetching messages page token={token}");

            // fetch the next 500 results
            let (b, res) = self
                .client
                .users()
                .messages_list("me")
                .include_spam_trash(true)
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

    // parses a single message from the Gmail API into a [`Message`] struct
    // also works on format=minimal
    fn parse_message(
        msg: google_gmail1::api::Message,
    ) -> Result<crate::email::repo::Message, AppError> {
        let Some(payload) = msg.payload.clone() else {
            // try minimal format
            match msg {
                google_gmail1::api::Message {
                    id: Some(msg_id),
                    label_ids: Some(label_ids),
                    ..
                } => {
                    return Ok(Message {
                        provider_msg_id: Some(msg_id),
                        label_ids,
                        ..Default::default()
                    })
                }
                _ => {
                    return Err(GmailError::MissingField(MissingField::Payload).into());
                }
            }
        };

        // parse headers
        let Some(headers) = payload.headers.clone() else {
            return Err(GmailError::MissingField(MissingField::Headers).into());
        };

        let mut header_map: HashMap<String, String> =
            HashMap::from_iter(headers.into_iter().filter_map(|it| it.name.zip(it.value)));

        let Some(msg_id) = msg.id.clone() else {
            return Err(GmailError::MissingField(MissingField::MsgId).into());
        };

        // TODO: implement tree-walking for this,
        // currently only supports one layer of container mesages
        let contents: Vec<MessageContents> = match payload {
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
                        error!("skipping message part inside msg_id={msg_id} err={e:?}");
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

        Ok(crate::email::repo::Message {
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
    ) -> Result<email::repo::MessageContents, AppError> {
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

        Ok(MessageContents {
            body,
            provider_msg_id,
            mime_type,
        })
    }

    pub async fn list_messages(
        &self,
        mailbox: MailBox,
        page_index: u32,
    ) -> Result<ListMessages, AppError> {
        self.repo.list_messages(mailbox, page_index).await
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
    MissingField(MissingField),
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

    let id = email::repo::add_account(
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
