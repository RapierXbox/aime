use std::sync::Arc;
use std::vec;
use std::{
    collections::HashMap, env::temp_dir, f64::consts::E, future, ops::ControlFlow, str::FromStr,
};

use base64::prelude::*;
use base64::Engine;
use google_gmail1::api::{MessagePartBody, MessagePartHeader};
use google_gmail1::{
    api::MessagePart,
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use log::error;
use log::warn;
use tauri::State;
use tokio::{fs::File, io::AsyncWriteExt, stream};

use crate::email;
use crate::email::gmail::repo::{GmailRepo, ListMessages};
use crate::email::repo::{Label, Message};
use crate::email::MailBox;
use crate::{
    email::{
        repo::{AccountConfig, EmailAccount, HistoryID, MessageContents, MissingField},
        EmailManager,
    },
    AppError, DbPool,
};

pub type GmailApiClient = google_gmail1::Gmail<HttpsConnector<HttpConnector>>;

pub mod auth;
mod drafts;
pub mod repo;
mod sync;

pub struct GmailClient {
    client: GmailApiClient,
    account_id: i64,
    email_addr: String,
    repo: GmailRepo,
    pub(super) draft_store: crate::email::drafts::store::Store,
    pub(super) draft_lock: tokio::sync::Mutex<()>,
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
            repo: GmailRepo::new(db_pool.clone(), account_id),
            draft_store: crate::email::drafts::store::Store(db_pool),
            draft_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn account_id(&self) -> i64 {
        self.account_id
    }

    pub fn email_addr(&self) -> &str {
        &self.email_addr
    }
    pub fn gmail_client(&self) -> &GmailApiClient {
        &self.client
    }

    // parses a single message from the Gmail API into a [`Message`] struct
    // also works on format=minimal
    pub(crate) fn parse_message(
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

        let mut header_issues = Vec::new();
        let mut header_map: HashMap<String, String> = HashMap::new();
        for (name, value) in headers.into_iter().filter_map(|h| h.name.zip(h.value)) {
            let key = name.to_ascii_lowercase();
            if header_map.contains_key(&key)
                && matches!(key.as_str(), "subject" | "from" | "message-id")
            {
                header_issues.push("Ambiguous message headers".into());
            }
            let separator = if matches!(key.as_str(), "to" | "cc" | "bcc" | "reply-to") {
                ", "
            } else {
                " "
            };
            header_map
                .entry(key)
                .and_modify(|v| {
                    v.push_str(separator);
                    v.push_str(&value);
                })
                .or_insert(value);
        }

        let Some(msg_id) = msg.id.clone() else {
            return Err(GmailError::MissingField(MissingField::MsgId).into());
        };

        let mut mime_issues = header_issues;
        if header_map
            .get("mime-version")
            .is_some_and(|v| v.trim() != "1.0")
        {
            mime_issues.push("Unsupported MIME version".into());
        }
        let contents = Self::collect_mime_leaves(payload, &msg_id, &mut mime_issues);
        let count = |mime: &str| contents.iter().filter(|c| c.mime_type == mime).count();
        if count("text/html") > 1 || count("text/plain") > 1 {
            // Re-uploading would keep only the first part of each type and drop the rest.
            mime_issues.push("Ambiguous message body".into());
        }

        let sync_cursor = msg.history_id.map(|id| id.to_string());
        let thread_id = msg.thread_id;

        let date_header = header_map.remove("date");
        let from_addr = header_map.remove("from");
        let to_addrs = header_map.remove("to");
        let cc_addrs = header_map.remove("cc");
        let subject = header_map.remove("subject").map(|value| {
            let raw = format!("Subject: {value}\r\n\r\n");
            mailparse::parse_headers(raw.as_bytes())
                .ok()
                .and_then(|(h, _)| h.first().map(|h| h.get_value()))
                .unwrap_or(value)
        });

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
            reply_to: header_map.remove("reply-to"),
            message_id: header_map.remove("message-id"),
            in_reply_to: header_map.remove("in-reply-to"),
            references: header_map.remove("references"),
            bcc_addrs: header_map.remove("bcc"),
            mime_issues,
            metadata_loaded: true,
        })
    }

    /// True for parts that are not body text: downloadable files and inline
    /// resources. These are fetched lazily by `attachmentId`, never stored as contents.
    fn is_attachment(part: &MessagePart) -> bool {
        match part {
            // Named file, e.g. `report.pdf`. Gmail sends `filename: ""` on body parts.
            MessagePart {
                filename: Some(f), ..
            } if !f.is_empty() => true,
            // Gmail moved the bytes out of the message; only fetchable separately.
            MessagePart {
                body:
                    Some(MessagePartBody {
                        attachment_id: Some(_),
                        ..
                    }),
                ..
            } => true,
            MessagePart {
                headers: Some(hs), ..
            } => hs.iter().any(Self::header_marks_attachment),
            _ => false,
        }
    }

    /// `Content-Disposition: attachment` (even without a filename), or any
    /// `Content-ID`, which marks an inline resource referenced from HTML as `cid:<id>`.
    fn header_marks_attachment(h: &MessagePartHeader) -> bool {
        let name = h.name.as_deref().unwrap_or_default().to_ascii_lowercase();
        match (name.as_str(), h.value.as_deref()) {
            ("content-disposition", Some(v)) => v.to_ascii_lowercase().contains("attachment"),
            ("content-id", _) => true,
            _ => false,
        }
    }

    /// Recursively walks a MIME part tree, collecting every leaf that parses.
    /// Attachments are skipped; container parts (`parts: Some(_)`) recurse;
    /// anything else is tried as a leaf. Problems are reported via `issues`.
    fn collect_mime_leaves(
        part: MessagePart,
        msg_id: &str,
        issues: &mut Vec<String>,
    ) -> Vec<MessageContents> {
        if Self::is_attachment(&part) {
            issues.push("Attachment or inline image".into());
            return vec![];
        }
        match (part.parts.is_some(), part.mime_type.as_deref()) {
            (true, Some(mime)) if !mime.starts_with("multipart/") => {
                issues.push("Ambiguous MIME container".into())
            }
            _ => {
                // do nothing
            }
        }
        match part.parts {
            Some(children) => {
                if children.is_empty() {
                    issues.push("Empty MIME container".into());
                }
                children
                    .into_iter()
                    .flat_map(|child| Self::collect_mime_leaves(child, msg_id, issues))
                    .collect()
            }
            None => match Self::parse_mime_leaf(part, msg_id.to_string()) {
                Ok(content) => vec![content],
                Err(_) => {
                    issues.push("Unsupported or incomplete message part".into());
                    vec![]
                }
            },
        }
    }

    /// If the MessagePart is a leaf part, parse into MessageContents
    /// filters accepted mime types and validates utf8
    fn parse_mime_leaf(
        part: MessagePart,
        provider_msg_id: String,
    ) -> Result<email::repo::MessageContents, AppError> {
        let mut part = part;
        if part.mime_type.as_deref() == Some("text/plain") {
            if let Some(body) = &mut part.body {
                if body.data.is_none() && body.size == Some(0) && body.attachment_id.is_none() {
                    body.data = Some(vec![]);
                }
            }
        }
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
        let body = String::from_utf8(body_data).map_err(|_| {
            error!("invalid UTF-8 message part for msg_id={provider_msg_id}");
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

    pub async fn get_message(&self, msg_id: &str) -> Result<Message, AppError> {
        self.repo.get_message(msg_id).await
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

#[cfg(test)]
mod compose_parser_tests {
    use super::*;
    use google_gmail1::api::{Message as GMessage, MessagePartHeader};
    fn text(data: &[u8]) -> MessagePart {
        MessagePart {
            mime_type: Some("text/plain".into()),
            body: Some(MessagePartBody {
                data: Some(data.to_vec()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    fn html(data: &[u8]) -> MessagePart {
        MessagePart {
            mime_type: Some("text/html".into()),
            ..text(data)
        }
    }
    fn parse(mut part: MessagePart) -> Message {
        part.headers.get_or_insert_default();
        GmailClient::parse_message(GMessage {
            id: Some("m".into()),
            payload: Some(part),
            ..Default::default()
        })
        .unwrap()
    }
    #[test]
    fn draft_classification_preserves_the_original_structure() {
        assert!(parse(text(b"already decoded")).mime_issues.is_empty());
        let mut empty = text(b"");
        empty.body.as_mut().unwrap().data = None;
        empty.body.as_mut().unwrap().size = Some(0);
        assert!(parse(empty).mime_issues.is_empty());
        let mut attachment = text(b"not compose text");
        attachment.filename = Some("notes.txt".into());
        assert!(parse(attachment.clone()).contents.is_empty());
        for part in [
            MessagePart {
                mime_type: Some("multipart/mixed".into()),
                parts: Some(vec![text(b"body"), attachment]),
                ..Default::default()
            },
            MessagePart {
                mime_type: Some("multipart/alternative".into()),
                parts: Some(vec![html(b"<p>one</p>"), html(b"<p>two</p>")]),
                ..Default::default()
            },
            MessagePart {
                mime_type: Some("multipart/mixed".into()),
                parts: Some(vec![text(b"one"), text(b"two")]),
                ..Default::default()
            },
            text(&[255]),
            MessagePart {
                mime_type: Some("text/plain".into()),
                body: Some(MessagePartBody {
                    attachment_id: Some("attachment".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ] {
            assert!(!parse(part).mime_issues.is_empty());
        }
    }
    #[test]
    fn plain_plus_html_alternative_has_no_issues() {
        let part = MessagePart {
            mime_type: Some("multipart/alternative".into()),
            parts: Some(vec![text(b"body"), html(b"<p>body</p>")]),
            ..Default::default()
        };
        assert!(parse(part).mime_issues.is_empty());
    }
    #[test]
    fn attachment_detected_by_headers() {
        let with_header = |name: &str, value: &str| MessagePart {
            headers: Some(vec![MessagePartHeader {
                name: Some(name.into()),
                value: Some(value.into()),
            }]),
            ..text(b"x")
        };
        assert!(GmailClient::is_attachment(&with_header(
            "Content-Disposition",
            "ATTACHMENT"
        )));
        assert!(GmailClient::is_attachment(&with_header(
            "content-id",
            "<img1>"
        )));
        assert!(!GmailClient::is_attachment(&with_header(
            "Content-Disposition",
            "inline"
        )));
        assert!(!GmailClient::is_attachment(&text(b"x")));
    }
    #[test]
    fn headers_are_case_insensitive_and_reply_ids_are_not_provider_ids() {
        let mut part = text(b"body");
        part.headers = Some(
            [
                ("sUbJeCt", "=?UTF-8?Q?Gr=C3=BC=C3=9Fe?="),
                ("reply-to", "you@example.com"),
                ("message-id", "<rfc@example.com>"),
                ("in-reply-to", "<parent@example.com>"),
                ("references", "<first@example.com>"),
                ("References", "<second@example.com>"),
                ("bcc", "hidden@example.com"),
            ]
            .map(|(n, v)| MessagePartHeader {
                name: Some(n.into()),
                value: Some(v.into()),
            })
            .to_vec(),
        );
        let p = parse(part);
        assert_eq!(p.subject.as_deref(), Some("Grüße"));
        assert_eq!(p.reply_to.as_deref(), Some("you@example.com"));
        assert_eq!(p.message_id.as_deref(), Some("<rfc@example.com>"));
        assert_eq!(
            p.references.as_deref(),
            Some("<first@example.com> <second@example.com>")
        );
        assert_eq!(p.bcc_addrs.as_deref(), Some("hidden@example.com"));
    }
}
