use std::sync::Arc;
use std::vec;
use std::{
    collections::HashMap, env::temp_dir, f64::consts::E, future, ops::ControlFlow, str::FromStr,
};

use base64::prelude::*;
use base64::Engine;
use google_gmail1::api::MessagePartBody;
use google_gmail1::{
    api::MessagePart,
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::error;
use log::warn;
use tauri::State;
use tokio::{fs::File, io::AsyncWriteExt, stream};

use crate::email::gmail::repo::{GmailRepo, ListMessages};
use crate::email::repo::{Label, Message};
use crate::email::MailBox;
use crate::email;
use crate::{
    email::{
        repo::{AccountConfig, EmailAccount, HistoryID, MessageContents, MissingField},
        EmailManager,
    },
    AppError, DbPool,
};

pub type GmailApiClient = google_gmail1::Gmail<HttpsConnector<HttpConnector>>;

pub mod auth;
pub mod repo;
mod sync;

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

        let contents = Self::collect_mime_leaves(payload, &msg_id);

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

    /// Recursively walks a MIME part tree, collecting every leaf that parses.
    /// Container parts (`parts: Some(_)`) recurse; anything else is tried as a leaf.
    fn collect_mime_leaves(part: MessagePart, msg_id: &str) -> Vec<MessageContents> {
        match part.parts {
            Some(children) => children
                .into_iter()
                .flat_map(|child| Self::collect_mime_leaves(child, msg_id))
                .collect(),

            None => Self::parse_mime_leaf(part, msg_id.to_string())
                .inspect_err(|e| error!("skipping message part inside msg_id={msg_id} err={e:?}"))
                .into_iter()
                .collect(),
        }
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

#[cfg(test)]
mod tests {
    use google_gmail1::api::{Message as GMessage, MessagePartHeader};

    use super::*;

    fn leaf(mime_type: &str, data: Option<Vec<u8>>) -> MessagePart {
        MessagePart {
            mime_type: Some(mime_type.into()),
            body: Some(MessagePartBody {
                data,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn parse_mime_leaf_invariants() {
        struct Case {
            name: &'static str,
            part: MessagePart,
            expect_ok: bool,
        }

        let cases = [
            Case {
                name: "text/plain is accepted",
                part: leaf("text/plain", Some(b"hello".to_vec())),
                expect_ok: true,
            },
            Case {
                name: "text/html is accepted",
                part: leaf("text/html", Some(b"<p>hi</p>".to_vec())),
                expect_ok: true,
            },
            Case {
                name: "unsupported mime type is rejected",
                part: leaf("image/png", Some(vec![1, 2, 3])),
                expect_ok: false,
            },
            Case {
                name: "invalid utf8 body is rejected",
                part: leaf("text/plain", Some(vec![0xff, 0xfe])),
                expect_ok: false,
            },
            Case {
                name: "missing body is not a leaf",
                part: MessagePart {
                    mime_type: Some("text/plain".into()),
                    body: None,
                    ..Default::default()
                },
                expect_ok: false,
            },
            Case {
                name: "missing body.data is not a leaf",
                part: leaf("text/plain", None),
                expect_ok: false,
            },
            Case {
                name: "missing mime_type is not a leaf",
                part: MessagePart {
                    mime_type: None,
                    body: Some(MessagePartBody {
                        data: Some(b"hello".to_vec()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                expect_ok: false,
            },
            Case {
                name: "container part (has sub-parts) is not a leaf",
                part: MessagePart {
                    parts: Some(vec![leaf("text/plain", Some(b"hi".to_vec()))]),
                    ..Default::default()
                },
                expect_ok: false,
            },
        ];

        for c in cases {
            let result = GmailClient::parse_mime_leaf(c.part, "msg1".into());
            assert_eq!(result.is_ok(), c.expect_ok, "case failed: {}", c.name);
        }
    }

    #[test]
    fn parse_message_invariants() {
        struct Case {
            name: &'static str,
            msg: GMessage,
            expect_ok: bool,
        }

        let full_payload = MessagePart {
            headers: Some(vec![MessagePartHeader {
                name: Some("Subject".into()),
                value: Some("hi".into()),
            }]),
            ..leaf("text/plain", Some(b"body".to_vec()))
        };

        let cases = [
            Case {
                name: "full payload with headers parses",
                msg: GMessage {
                    id: Some("m1".into()),
                    payload: Some(full_payload.clone()),
                    ..Default::default()
                },
                expect_ok: true,
            },
            Case {
                name: "payload with nested parts parses",
                msg: GMessage {
                    id: Some("m1".into()),
                    payload: Some(MessagePart {
                        headers: Some(vec![]),
                        parts: Some(vec![leaf("text/plain", Some(b"a".to_vec()))]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                expect_ok: true,
            },
            Case {
                name: "minimal format (no payload) with id + labels parses",
                msg: GMessage {
                    id: Some("m1".into()),
                    label_ids: Some(vec!["INBOX".into()]),
                    payload: None,
                    ..Default::default()
                },
                expect_ok: true,
            },
            Case {
                name: "no payload and no id/labels errors",
                msg: GMessage {
                    payload: None,
                    ..Default::default()
                },
                expect_ok: false,
            },
            Case {
                name: "no payload with id but no labels errors",
                msg: GMessage {
                    id: Some("m1".into()),
                    label_ids: None,
                    payload: None,
                    ..Default::default()
                },
                expect_ok: false,
            },
            Case {
                name: "payload without headers errors",
                msg: GMessage {
                    id: Some("m1".into()),
                    payload: Some(MessagePart {
                        headers: None,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                expect_ok: false,
            },
            Case {
                name: "payload+headers but missing message id errors",
                msg: GMessage {
                    id: None,
                    payload: Some(full_payload.clone()),
                    ..Default::default()
                },
                expect_ok: false,
            },
        ];

        for c in cases {
            let result = GmailClient::parse_message(c.msg);
            assert_eq!(result.is_ok(), c.expect_ok, "case failed: {}", c.name);
        }
    }

    #[test]
    fn parse_message_walks_nested_mime_tree() {
        let nested_container = MessagePart {
            mime_type: Some("multipart/alternative".into()),
            parts: Some(vec![
                leaf("text/plain", Some(b"plain body".to_vec())),
                leaf("text/html", Some(b"<p>html body</p>".to_vec())),
            ]),
            ..Default::default()
        };

        let msg = GMessage {
            id: Some("m1".into()),
            payload: Some(MessagePart {
                mime_type: Some("multipart/mixed".into()),
                headers: Some(vec![]),
                parts: Some(vec![nested_container]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let parsed = GmailClient::parse_message(msg).expect("message should parse");
        assert_eq!(
            parsed.contents.len(),
            2,
            "expected both nested leaf parts to survive tree-walking, got {:?}",
            parsed.contents
        );
    }
}
