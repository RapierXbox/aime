use super::GmailClient;
use crate::email::drafts::{model::*, service::*, store::Attempt};
use async_trait::async_trait;
use google_gmail1::api::{Draft as GmailDraft, Message as GmailMessage};
use std::{io::Cursor, time::Duration};

fn error(e: google_gmail1::Error) -> ProviderError {
    match e {
        google_gmail1::Error::MissingToken(e) => {
            if matches!(
                e.downcast_ref::<crate::AppError>(),
                Some(crate::AppError::EmailNetworkError)
            ) {
                ProviderError::Unknown
            } else {
                ProviderError::Authorization
            }
        }
        google_gmail1::Error::BadRequest(_) | google_gmail1::Error::UploadSizeLimitExceeded(..) => {
            ProviderError::Rejected
        }
        google_gmail1::Error::Failure(r) if matches!(r.status().as_u16(), 401 | 403) => {
            ProviderError::Authorization
        }
        google_gmail1::Error::Failure(r)
            if r.status().is_client_error() && r.status().as_u16() != 408 =>
        {
            ProviderError::Rejected
        }
        _ => ProviderError::Unknown,
    }
}
fn parse_remote(d: GmailDraft) -> Result<RemoteDraft, ProviderError> {
    let id = d.id.ok_or(ProviderError::Unknown)?;
    let msg = d.message.ok_or(ProviderError::Unknown)?;
    let message_id = msg.id.clone().unwrap_or_else(|| format!("incomplete:{id}"));
    let operation_id = msg
        .payload
        .as_ref()
        .and_then(|p| p.headers.as_ref())
        .and_then(|hs| {
            hs.iter()
                .find(|h| {
                    h.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case("X-Aime-Operation"))
                })
                .and_then(|h| h.value.clone())
        });
    let mut p = GmailClient::parse_message(msg).unwrap_or_else(|_| crate::email::repo::Message {
        mime_issues: vec!["Incomplete Gmail message".into()],
        ..Default::default()
    });
    if !p.metadata_loaded {
        p.mime_issues.push("Incomplete Gmail message".into());
    }
    let part = |mime: &str| {
        p.contents
            .iter()
            .find(|c| c.mime_type == mime)
            .map(|c| c.body.as_str())
    };
    let body_text = part("text/plain").unwrap_or_default().to_string();
    let (body_html, quote_html) = match part("text/html") {
        Some(html) => split_gmail_quote(html),
        None => (&*text_to_html(&body_text), ""),
    };
    let fields = DraftFields {
        to: p.to_addrs.unwrap_or_default(),
        cc: p.cc_addrs.unwrap_or_default(),
        bcc: p.bcc_addrs.unwrap_or_default(),
        subject: p.subject.unwrap_or_default(),
        body_html: body_html.to_string(),
        quote_html: quote_html.to_string(),
        body_text,
    };
    let mut issues = p.mime_issues;
    if let Some(reply_to) = &p.reply_to {
        let mailboxes = |value: &str| {
            parse_addresses(value).ok().map(|a| {
                a.into_iter()
                    .map(|(_, email)| email.to_ascii_lowercase())
                    .collect::<Vec<_>>()
            })
        };
        if mailboxes(reply_to) != mailboxes(p.from_addr.as_deref().unwrap_or("")) {
            issues.push("Custom Reply-To cannot be preserved by this composer".into());
        }
    }
    if !crate::email::drafts::model::valid_reply_headers(&p.in_reply_to, &p.references) {
        issues.push("Unsupported reply headers".into());
    }
    if parse_addresses(&fields.to).is_err()
        || parse_addresses(&fields.cc).is_err()
        || parse_addresses(&fields.bcc).is_err()
    {
        issues.push("Unsupported address headers".into());
    }
    let from = parse_addresses(p.from_addr.as_deref().unwrap_or(""))
        .ok()
        .and_then(|a| {
            if a.len() == 1 {
                Some(a[0].1.clone())
            } else {
                None
            }
        })
        .unwrap_or_default();
    Ok(RemoteDraft {
        id,
        message_id,
        fields,
        issues,
        thread_id: p.thread_id,
        in_reply_to: p.in_reply_to,
        references: p.references,
        operation_id,
        from,
    })
}
#[async_trait]
impl Provider for GmailClient {
    async fn ready(&self) -> Result<(), ProviderError> {
        // Read-only preflight: failed connectivity/auth never queues a send.
        let (_, profile) = tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .users()
                .get_profile("me")
                .add_scope("https://www.googleapis.com/auth/gmail.modify")
                .doit(),
        )
        .await
        .map_err(|_| ProviderError::Unknown)?
        .map_err(error)?;
        if !profile
            .email_address
            .as_deref()
            .is_some_and(|email| email.eq_ignore_ascii_case(&self.email_addr))
        {
            return Err(ProviderError::Authorization);
        }
        Ok(())
    }
    async fn list(&self) -> Result<Vec<RemoteDraft>, ProviderError> {
        let cached = self
            .draft_store
            .list(self.account_id)
            .await
            .map_err(|e| {
                log::error!("reading local drafts for account {} failed: {e}", self.account_id);
                ProviderError::Unknown
            })?;
        let mut result = vec![];
        let mut token: Option<String> = None;
        loop {
            let mut call = self.client.users().drafts_list("me").max_results(500);
            if let Some(t) = &token {
                call = call.page_token(t);
            }
            let (_, page) = tokio::time::timeout(Duration::from_secs(30), call.doit())
                .await
                .map_err(|_| ProviderError::Unknown)?
                .map_err(error)?;
            for stub in page.drafts.unwrap_or_default() {
                if let Some(id) = stub.id {
                    let message_id = stub.message.as_ref().and_then(|m| m.id.as_ref());
                    let known = cached
                        .iter()
                        .filter(|d| d.attempt_id.is_none())
                        .filter_map(|d| d.remote.as_ref())
                        .find(|r| r.id == id && Some(&r.message_id) == message_id);
                    if let Some(r) = known {
                        result.push(r.clone());
                    } else if let Some(d) = self.get(&id).await? {
                        result.push(d);
                    }
                } else {
                    return Err(ProviderError::Unknown);
                }
            }
            token = page.next_page_token;
            if token.is_none() {
                break;
            }
        }
        Ok(result)
    }
    async fn get(&self, id: &str) -> Result<Option<RemoteDraft>, ProviderError> {
        let r = tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .users()
                .drafts_get("me", id)
                .format("full")
                .doit(),
        )
        .await
        .map_err(|_| ProviderError::Unknown)?;
        match r {
            Ok((_, d)) => Ok(Some(parse_remote(d)?)),
            Err(google_gmail1::Error::Failure(r)) if r.status().as_u16() == 404 => Ok(None),
            Err(e) => Err(error(e)),
        }
    }
    async fn mutate(&self, a: &Attempt) -> Result<Receipt, ProviderError> {
        let request = GmailDraft {
            id: a.remote_id.clone(),
            message: Some(GmailMessage {
                thread_id: a.draft.thread_id.clone(),
                ..Default::default()
            }),
        };
        let mime_type = "message/rfc822".parse().unwrap();
        let reader = Cursor::new(a.mime.clone());
        let result = match a.kind.as_str() {
            "create" => {
                let (_, d) = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.client
                        .users()
                        .drafts_create(request, "me")
                        .upload(reader, mime_type),
                )
                .await
                .map_err(|_| ProviderError::Unknown)?
                .map_err(error)?;
                let id = d.id.ok_or(ProviderError::Unknown)?;
                let uploaded_id = d.message.and_then(|m| m.id).ok_or(ProviderError::Unknown)?;
                // Response can be minimal. Detail-fetch failure remains uncertain and reconciles by operation marker.
                Receipt {
                    remote: Some(
                        self.get(&id)
                            .await
                            .map_err(|_| ProviderError::Unknown)?
                            .ok_or(ProviderError::Unknown)?,
                    ),
                    sent_id: None,
                    uploaded_id: Some(uploaded_id),
                }
            }
            "update" => {
                let id = a.remote_id.as_deref().ok_or(ProviderError::Rejected)?;
                let (_, d) = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.client
                        .users()
                        .drafts_update(request, "me", id)
                        .upload(reader, mime_type),
                )
                .await
                .map_err(|_| ProviderError::Unknown)?
                .map_err(error)?;
                let uploaded_id = d.message.and_then(|m| m.id).ok_or(ProviderError::Unknown)?;
                Receipt {
                    remote: Some(
                        self.get(id)
                            .await
                            .map_err(|_| ProviderError::Unknown)?
                            .ok_or(ProviderError::Unknown)?,
                    ),
                    sent_id: None,
                    uploaded_id: Some(uploaded_id),
                }
            }
            "send" => {
                let (_, sent) = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.client
                        .users()
                        .drafts_send(request, "me")
                        .upload(reader, mime_type),
                )
                .await
                .map_err(|_| ProviderError::Unknown)?
                .map_err(error)?;
                Receipt {
                    remote: None,
                    sent_id: Some(sent.id.ok_or(ProviderError::Unknown)?),
                    uploaded_id: None,
                }
            }
            "delete" => {
                let id = a.remote_id.as_deref().ok_or(ProviderError::Rejected)?;
                let res = tokio::time::timeout(
                    Duration::from_secs(30),
                    self.client.users().drafts_delete("me", id).doit(),
                )
                .await
                .map_err(|_| ProviderError::Unknown)?;
                match res {
                    Ok(_) => {}
                    Err(google_gmail1::Error::Failure(r)) if r.status().as_u16() == 404 => {}
                    Err(e) => return Err(error(e)),
                }
                Receipt {
                    remote: None,
                    sent_id: None,
                    uploaded_id: None,
                }
            }
            _ => return Err(ProviderError::Rejected),
        };
        Ok(result)
    }
    async fn find_sent(&self, operation: &str) -> Result<Option<String>, ProviderError> {
        let (_, result) = tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .users()
                .messages_list("me")
                .q(&format!("in:sent rfc822msgid:{operation}@aime.local"))
                .doit(),
        )
        .await
        .map_err(|_| ProviderError::Unknown)?
        .map_err(error)?;
        let matches = result.messages.unwrap_or_default();
        if matches.len() != 1 || result.next_page_token.is_some() {
            return Ok(None);
        }
        Ok(matches[0].id.clone())
    }
    async fn source(&self, id: &str) -> Result<crate::email::repo::Message, ProviderError> {
        if let Ok(cached) = self.repo.get_message(id).await {
            if cached.metadata_loaded && !cached.contents.is_empty() {
                return Ok(cached);
            }
        }
        let (_, msg) = tokio::time::timeout(
            Duration::from_secs(30),
            self.client
                .users()
                .messages_get("me", id)
                .format("full")
                .doit(),
        )
        .await
        .map_err(|_| ProviderError::Unknown)?
        .map_err(error)?;
        let parsed = Self::parse_message(msg).map_err(|_| ProviderError::Rejected)?;
        // Enrich a cached source without making its availability depend on cache hydration.
        if let Ok(copy) = serde_json::from_value(serde_json::to_value(&parsed).unwrap_or_default())
        {
            let _ = self.repo.backfill_message(copy).await;
        }
        Ok(parsed)
    }
}

impl GmailClient {
    pub(crate) async fn cache_sent(&self, id: &str) -> Result<(), crate::AppError> {
        self.sync_labels().await?;
        let (_, message) = self
            .client
            .users()
            .messages_get("me", id)
            .format("full")
            .doit()
            .await?;
        self.repo
            .insert_skeleton_messages(&[message.clone()])
            .await?;
        self.repo
            .backfill_message(Self::parse_message(message)?)
            .await
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use crate::{
        email::drafts::{service::Service, store::Store},
        DbPool,
    };
    use google_gmail1::{
        common::GetToken,
        hyper_rustls::HttpsConnectorBuilder,
        hyper_util::{client::legacy::Client, rt::TokioExecutor},
    };
    use std::{
        future::Future,
        pin::Pin,
        sync::{Arc, Mutex},
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[derive(Clone)]
    struct Token;
    impl GetToken for Token {
        fn get_token<'a>(
            &'a self,
            _: &'a [&str],
        ) -> Pin<
            Box<
                dyn Future<
                        Output = Result<Option<String>, Box<dyn std::error::Error + Send + Sync>>,
                    > + Send
                    + 'a,
            >,
        > {
            Box::pin(async { Ok(Some("test-token".into())) })
        }
    }
    async fn client(
        responses: Vec<(u16, serde_json::Value)>,
    ) -> (
        GmailClient,
        Arc<Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(vec![]));
        let capture = captured.clone();
        let server = tokio::spawn(async move {
            for (status, body) in responses {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut bytes = vec![];
                loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let size = headers
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|s| s.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + size {
                            break;
                        }
                    }
                }
                capture
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&bytes).into_owned());
                let data = body.to_string();
                let reply=format!("HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len());
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let http = Client::builder(TokioExecutor::new()).build(
            HttpsConnectorBuilder::new()
                .with_native_roots()
                .unwrap()
                .https_or_http()
                .enable_http1()
                .enable_http2()
                .build(),
        );
        let mut api = super::super::GmailApiClient::new(http, Token);
        api.base_url(format!("http://{address}/gmail/v1/"));
        api.root_url(format!("http://{address}/"));
        let db = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!().run(&db).await.unwrap();
        sqlx::query("INSERT INTO email_accounts(id,email,account_name,account_config) VALUES(1,'me@example.com','Test','{}')").execute(&db).await.unwrap();
        (
            GmailClient::new(1, db, "me@example.com".into(), api),
            captured,
            server,
        )
    }
    fn full(id: &str, operation: Option<&str>) -> serde_json::Value {
        use base64::{prelude::BASE64_URL_SAFE, Engine};
        let mut headers = vec![
            serde_json::json!({"name":"From","value":"me@example.com"}),
            serde_json::json!({"name":"To","value":"you@example.com"}),
            serde_json::json!({"name":"Subject","value":"Hello"}),
        ];
        if let Some(op) = operation {
            headers.push(serde_json::json!({"name":"X-Aime-Operation","value":op}));
        }
        serde_json::json!({"id":id,"message":{"id":"provider-message","payload":{"mimeType":"text/plain","headers":headers,"body":{"size":5,"data":BASE64_URL_SAFE.encode(b"hello")}}}})
    }
    fn alternative(parts: &[(&str, &str)]) -> GmailDraft {
        use base64::{prelude::BASE64_URL_SAFE, Engine};
        let parts: Vec<_> = parts
            .iter()
            .map(|(mime, body)| serde_json::json!({"mimeType":mime,"body":{"size":body.len(),"data":BASE64_URL_SAFE.encode(body)}}))
            .collect();
        serde_json::from_value(serde_json::json!({"id":"d","message":{"id":"m","payload":{"mimeType":"multipart/alternative","headers":[
            {"name":"From","value":"me@example.com"},{"name":"To","value":"you@example.com"},{"name":"Subject","value":"Hi"}],"parts":parts}}})).unwrap()
    }
    #[test]
    fn gmail_html_draft_is_split_at_the_quote_and_editable() {
        let html =
            r#"<div dir="ltr">hi</div><div class="gmail_quote"><blockquote>q</blockquote></div>"#;
        let r = parse_remote(alternative(&[("text/plain", "hi"), ("text/html", html)])).unwrap();
        assert!(r.issues.is_empty());
        assert_eq!(r.fields.body_html, r#"<div dir="ltr">hi</div>"#);
        assert!(r
            .fields
            .quote_html
            .starts_with(r#"<div class="gmail_quote">"#));
        assert_eq!(r.fields.body_text, "hi");
    }
    #[test]
    fn text_only_remote_draft_gets_escaped_html_body() {
        let r = parse_remote(alternative(&[("text/plain", "a < b\nc")])).unwrap();
        assert_eq!(r.fields.body_html, "a &lt; b<br>c");
        assert!(r.fields.quote_html.is_empty());
    }
    #[test]
    fn two_html_parts_are_ambiguous() {
        let r = parse_remote(alternative(&[
            ("text/html", "<p>a</p>"),
            ("text/html", "<p>b</p>"),
        ]))
        .unwrap();
        assert!(r.issues.iter().any(|i| i == "Ambiguous message body"));
    }
    #[tokio::test]
    async fn production_list_paginates_and_decodes_body_once() {
        let _: GmailDraft = serde_json::from_value(full("one", None)).unwrap();
        let (c, requests, server) = client(vec![
            (
                200,
                serde_json::json!({"drafts":[{"id":"one"}],"nextPageToken":"next"}),
            ),
            (200, full("one", None)),
            (200, serde_json::json!({"drafts":[]})),
        ])
        .await;
        let drafts = c.list().await.unwrap();
        assert_eq!(drafts[0].fields.body_text, "hello");
        assert!(drafts[0].issues.is_empty());
        server.await.unwrap();
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].contains("pageToken=next"));
    }
    #[tokio::test]
    async fn production_partial_list_failure_keeps_existing_draft() {
        let (c, _, server) = client(vec![
            (200, serde_json::json!({"drafts":[],"nextPageToken":"next"})),
            (
                503,
                serde_json::json!({"error":{"code":503,"message":"Unavailable"}}),
            ),
        ])
        .await;
        let s = c.draft_store.clone();
        let d = Draft::new(1, DraftFields::default());
        s.insert(&d).await.unwrap();
        let service = Service {
            store: s.clone(),
            provider: &c,
            sender: c.email_addr(),
            account: 1,
        };
        assert!(service.sync().await.is_err());
        assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Local);
        server.await.unwrap();
    }
    #[tokio::test]
    async fn confirmed_create_followed_by_denied_get_remains_unknown() {
        let (c, requests, server) = client(vec![
            (200, serde_json::json!({"emailAddress":"me@example.com"})),
            (
                200,
                serde_json::json!({"id":"created","message":{"id":"m"}}),
            ),
            (
                403,
                serde_json::json!({"error":{"code":403,"message":"Denied"}}),
            ),
        ])
        .await;
        let s = c.draft_store.clone();
        let d = Draft::new(
            1,
            DraftFields {
                to: "you@example.com".into(),
                subject: "Hello".into(),
                body_text: "hello".into(),
                ..Default::default()
            },
        );
        s.insert(&d).await.unwrap();
        let service = Service {
            store: s.clone(),
            provider: &c,
            sender: c.email_addr(),
            account: 1,
        };
        assert!(matches!(
            service.mutation(d.clone(), "upload").await,
            Err(crate::AppError::EmailOutcomeUnknown)
        ));
        assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Unknown);
        assert!(s.pending(&d.id).await.unwrap().is_some());
        server.await.unwrap();
        let requests = requests.lock().unwrap();
        assert!(requests[1].starts_with("POST /upload/gmail/v1/users/me/drafts"));
        assert!(requests[1].contains("X-Aime-Operation:"));
    }
    #[tokio::test]
    async fn production_send_submits_exact_mime_and_remote_id() {
        let (c, requests, server) = client(vec![(
            200,
            serde_json::json!({"id":"sent-message","labelIds":["SENT"]}),
        )])
        .await;
        let d = Draft::new(
            1,
            DraftFields {
                to: "you@example.com".into(),
                bcc: "private@example.com".into(),
                subject: "Reviewed".into(),
                body_text: "Reviewed text".into(),
                ..Default::default()
            },
        );
        let a = Attempt {
            id: "operation".into(),
            kind: "send".into(),
            draft: d.clone(),
            remote_id: Some("remote-draft".into()),
            mime: build_mime_message(&d, "me@example.com", "operation", true).unwrap(),
        };
        let receipt = c.mutate(&a).await.unwrap();
        assert_eq!(receipt.sent_id.as_deref(), Some("sent-message"));
        server.await.unwrap();
        let request = requests.lock().unwrap();
        assert!(request[0].contains("remote-draft"));
        assert!(request[0].contains("Reviewed text"));
        assert!(request[0].contains("private@example.com"));
        assert!(request[0].contains("operation@aime.local"));
    }
    #[tokio::test]
    async fn reply_fetches_full_source_and_enriches_old_cached_metadata() {
        let mut response = full("remote", None)["message"].clone();
        response["threadId"] = serde_json::json!("thread");
        let headers = response["payload"]["headers"].as_array_mut().unwrap();
        headers[0]["value"] = serde_json::json!("person@example.com");
        headers.push(serde_json::json!({"name":"Message-ID","value":"<source@example.com>"}));
        headers.push(serde_json::json!({"name":"Reply-To","value":"replies@example.com"}));
        let (c, requests, server) = client(vec![(200, response)]).await;
        sqlx::query("INSERT INTO messages(account_id,provider_msg_id,internal_date) VALUES(1,'provider-message',1)").execute(&c.draft_store.0).await.unwrap();
        let svc = Service {
            store: c.draft_store.clone(),
            provider: &c,
            sender: c.email_addr(),
            account: 1,
        };
        let draft = svc
            .create(ComposeMode::Reply, Some("provider-message".into()))
            .await
            .unwrap();
        assert_eq!(draft.fields.to, "replies@example.com");
        assert_eq!(draft.in_reply_to.as_deref(), Some("<source@example.com>"));
        let cached = c.repo.get_message("provider-message").await.unwrap();
        assert!(cached.metadata_loaded);
        assert_eq!(cached.message_id.as_deref(), Some("<source@example.com>"));
        server.await.unwrap();
        assert!(requests.lock().unwrap()[0].contains("format=full"));
    }
    #[tokio::test]
    async fn wrong_authenticated_account_cannot_submit_mail() {
        let (c, requests, server) = client(vec![(
            200,
            serde_json::json!({"emailAddress":"different@example.com"}),
        )])
        .await;
        let d = Draft::new(
            1,
            DraftFields {
                to: "you@example.com".into(),
                ..Default::default()
            },
        );
        c.draft_store.insert(&d).await.unwrap();
        let svc = Service {
            store: c.draft_store.clone(),
            provider: &c,
            sender: c.email_addr(),
            account: 1,
        };
        assert!(matches!(
            svc.send(&d.id, d.revision).await,
            Err(crate::AppError::EmailAuthorizationRequired)
        ));
        assert!(c.draft_store.pending(&d.id).await.unwrap().is_none());
        server.await.unwrap();
        assert_eq!(requests.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn complete_cached_source_can_be_used_offline() {
        let (c, requests, server) = client(vec![]).await;
        let metadata=serde_json::json!({"metadata_loaded":true,"message_id":"<cached@example.com>","reply_to":"replies@example.com"}).to_string();
        sqlx::query("INSERT INTO messages(account_id,provider_msg_id,internal_date,from_addr,subject,compose_metadata) VALUES(1,'cached',1,'sender@example.com','Cached',?)").bind(metadata).execute(&c.draft_store.0).await.unwrap();
        sqlx::query("INSERT INTO message_contents(account_id,provider_msg_id,mime_type,body) VALUES(1,'cached','text/plain','Offline body')").execute(&c.draft_store.0).await.unwrap();
        server.await.unwrap();
        let svc = Service {
            store: c.draft_store.clone(),
            provider: &c,
            sender: c.email_addr(),
            account: 1,
        };
        let draft = svc
            .create(ComposeMode::Reply, Some("cached".into()))
            .await
            .unwrap();
        assert_eq!(draft.fields.to, "replies@example.com");
        assert!(draft.fields.quote_html.contains("Offline body"));
        assert!(requests.lock().unwrap().is_empty());
    }
}
