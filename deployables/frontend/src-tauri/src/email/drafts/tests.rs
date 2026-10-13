use super::{model::*, service::*, store::*};
use crate::{email::repo::Message, AppError, DbPool};
use async_trait::async_trait;
use std::sync::Mutex;

#[derive(Default)]
struct Fake {
    remote: Mutex<Vec<RemoteDraft>>,
    sent: Mutex<Vec<(String, String)>>,
    calls: Mutex<Vec<Attempt>>,
    failure: Mutex<Option<&'static str>>,
    offline: bool,
    bad_scan: bool,
    during_upload: Option<Store>,
    outside_upload: bool,
    /// Gmail returns the uploaded MIME text with CRLF line endings.
    wire_crlf: bool,
}
fn remote(id: &str, fields: DraftFields) -> RemoteDraft {
    RemoteDraft {
        id: id.into(),
        message_id: format!("m-{id}"),
        fields,
        issues: vec![],
        thread_id: None,
        in_reply_to: None,
        references: None,
        operation_id: None,
        from: "me@example.com".into(),
    }
}
#[async_trait]
impl Provider for Fake {
    async fn ready(&self) -> Result<(), ProviderError> {
        if self.offline {
            Err(ProviderError::Unknown)
        } else {
            Ok(())
        }
    }
    async fn list(&self) -> Result<Vec<RemoteDraft>, ProviderError> {
        if self.bad_scan {
            Err(ProviderError::Unknown)
        } else {
            Ok(self.remote.lock().unwrap().clone())
        }
    }
    async fn get(&self, id: &str) -> Result<Option<RemoteDraft>, ProviderError> {
        Ok(self
            .remote
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .cloned())
    }
    async fn source(&self, _: &str) -> Result<Message, ProviderError> {
        Ok(Message {
            message_id: Some("<source@example.com>".into()),
            from_addr: Some("person@example.com".into()),
            ..Default::default()
        })
    }
    async fn find_sent(&self, op: &str) -> Result<Option<String>, ProviderError> {
        Ok(self
            .sent
            .lock()
            .unwrap()
            .iter()
            .find(|(a, _)| a == op)
            .map(|(_, id)| id.clone()))
    }
    async fn mutate(&self, a: &Attempt) -> Result<Receipt, ProviderError> {
        self.calls.lock().unwrap().push(a.clone());
        let failure = self.failure.lock().unwrap().take();
        if failure == Some("rejected") {
            return Err(ProviderError::Rejected);
        }
        if failure == Some("unknown-before") {
            return Err(ProviderError::Unknown);
        }
        if let Some(store) = &self.during_upload {
            if matches!(a.kind.as_str(), "create" | "update") {
                let d = store.get(&a.draft.id).await.unwrap();
                let mut fields = d.fields.clone();
                fields.body_text = "new local edit".into();
                store.save(&d.id, d.revision, fields).await.unwrap();
            }
        }
        let mut r = remote(
            a.remote_id.as_deref().unwrap_or(&a.id),
            a.draft.fields.clone(),
        );
        r.operation_id = Some(a.id.clone());
        r.message_id = format!("message-{}", a.id);
        if self.wire_crlf {
            r.fields.body_text = r.fields.body_text.replace('\n', "\r\n");
        }
        if self.outside_upload {
            r.fields.body_text = "Gmail changed this".into();
            r.issues.push("Attachment or inline image".into());
        }
        let mut remotes = self.remote.lock().unwrap();
        match a.kind.as_str() {
            "send" => {
                remotes.retain(|r| Some(&r.id) != a.remote_id.as_ref());
                self.sent
                    .lock()
                    .unwrap()
                    .push((a.id.clone(), "sent-1".into()));
            }
            "delete" => remotes.retain(|r| Some(&r.id) != a.remote_id.as_ref()),
            _ => {
                remotes.retain(|old| old.id != r.id);
                remotes.push(r.clone());
            }
        }
        if failure == Some("unknown-after") {
            return Err(ProviderError::Unknown);
        }
        let uploaded_id = if self.outside_upload {
            "gmail-edit".to_string()
        } else {
            r.message_id.clone()
        };
        let draft_write = !matches!(a.kind.as_str(), "send" | "delete");
        Ok(Receipt {
            remote: if draft_write { Some(r) } else { None },
            uploaded_id: draft_write.then_some(uploaded_id),
            sent_id: if a.kind == "send" {
                Some("sent-1".into())
            } else {
                None
            },
        })
    }
}
async fn store() -> Store {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    for id in [1, 2] {
        sqlx::query(
            "INSERT INTO email_accounts(id,email,account_name,account_config) VALUES(?,?,?,'{}')",
        )
        .bind(id)
        .bind(format!("acc{id}@example.com"))
        .bind("test")
        .execute(&pool)
        .await
        .unwrap();
    }
    Store(pool)
}
fn service<'a>(store: &Store, fake: &'a Fake) -> Service<'a> {
    Service {
        store: store.clone(),
        provider: fake,
        sender: "me@example.com",
        account: 1,
    }
}
fn fields() -> DraftFields {
    DraftFields {
        to: "you@example.com".into(),
        body_text: "hello".into(),
        body_html: "hello".into(),
        ..Default::default()
    }
}
async fn local(store: &Store) -> Draft {
    let d = Draft::new(1, fields());
    store.insert(&d).await.unwrap();
    d
}

#[tokio::test]
async fn save_revision_and_account_isolation() {
    let s = store().await;
    let d = local(&s).await;
    assert!(s.list(2).await.unwrap().is_empty());
    let mut unfinished = d.fields.clone();
    unfinished.to = "partial@".into();
    let saved = s.save(&d.id, d.revision, unfinished.clone()).await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().fields, unfinished);
    assert!(matches!(
        s.save(&d.id, d.revision, fields()).await,
        Err(AppError::DraftRevisionConflict)
    ));
    assert_eq!(saved.revision, 2);
}
#[tokio::test]
async fn unchanged_upload_then_outside_edit_imports_and_changes_id() {
    let s = store().await;
    let f = Fake::default();
    let svc = service(&s, &f);
    let d = local(&s).await;
    svc.sync().await.unwrap();
    let synced = s.get(&d.id).await.unwrap();
    assert_eq!(synced.status, DraftStatus::Synced);
    {
        let mut r = f.remote.lock().unwrap();
        r[0].message_id = "new-id".into();
        r[0].fields.subject = "Gmail edit".into();
    }
    svc.sync().await.unwrap();
    let changed = s.get(&d.id).await.unwrap();
    assert_eq!(changed.id, d.id);
    assert_eq!(changed.fields.subject, "Gmail edit");
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn dirty_local_and_remote_conflict_preserves_both_and_copy() {
    let s = store().await;
    let f = Fake::default();
    let svc = service(&s, &f);
    let d = local(&s).await;
    svc.sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    let mut fields = d.fields.clone();
    fields.body_text = "local version".into();
    s.save(&d.id, d.revision, fields).await.unwrap();
    {
        let mut r = f.remote.lock().unwrap();
        r[0].message_id = "outside".into();
        r[0].fields.body_text = "remote version".into();
    }
    svc.sync().await.unwrap();
    let conflicted = s.get(&d.id).await.unwrap();
    assert_eq!(conflicted.status, DraftStatus::Conflict);
    assert_eq!(conflicted.fields.body_text, "local version");
    assert_eq!(
        conflicted.conflict.as_ref().unwrap().fields.body_text,
        "remote version"
    );
    let copy = svc
        .conflict(
            &d.id,
            conflicted.revision,
            ConflictChoice::KeepLocalAsCopy,
            None,
        )
        .await
        .unwrap();
    assert_ne!(copy.id, d.id);
    assert!(copy.remote.is_none());
    assert_eq!(copy.fields.body_text, "local version");
    assert_eq!(
        s.get(&d.id).await.unwrap().fields.body_text,
        "remote version"
    );
}
#[tokio::test]
async fn failed_scan_does_not_infer_deletion_and_remote_missing_does_not_recreate() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let failing = Fake {
        bad_scan: true,
        ..Default::default()
    };
    assert!(service(&s, &failing).sync().await.is_err());
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
    f.remote.lock().unwrap().clear();
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Missing);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn unknown_create_and_update_reconcile_without_reissuing() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).sync().await.is_err());
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Unknown);
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert_eq!(s.list(1).await.unwrap().len(), 1);
    let d = s.get(&d.id).await.unwrap();
    let mut fields = d.fields.clone();
    fields.subject = "updated".into();
    s.save(&d.id, d.revision, fields).await.unwrap();
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).sync().await.is_err());
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
    assert_eq!(f.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn unresolved_operation_does_not_retry_on_repeated_sync() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    *f.failure.lock().unwrap() = Some("unknown-before");
    assert!(service(&s, &f).sync().await.is_err());
    service(&s, &f).sync().await.unwrap();
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Unknown);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn edit_during_upload_remains_dirty_and_is_not_acknowledged() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    let mut field = d.fields.clone();
    field.body_text = "submitted".into();
    s.save(&d.id, d.revision, field).await.unwrap();
    let f = Fake {
        remote: Mutex::new(f.remote.lock().unwrap().clone()),
        during_upload: Some(s.clone()),
        ..Default::default()
    };
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    assert_eq!(d.fields.body_text, "new local edit");
    assert_eq!(d.remote.unwrap().fields.body_text, "submitted");
    assert_eq!(d.status, DraftStatus::Local);
}
#[tokio::test]
async fn unknown_send_never_retries_and_confirmed_receipt_locks_draft() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(matches!(
        service(&s, &f).send(&d.id, d.revision).await,
        Err(AppError::EmailOutcomeUnknown)
    ));
    let unknown = s.get(&d.id).await.unwrap();
    assert_eq!(unknown.status, DraftStatus::Unknown);
    assert!(s.save(&d.id, unknown.revision, fields()).await.is_err());
    assert!(service(&s, &f).send(&d.id, unknown.revision).await.is_err());
    service(&s, &f).sync().await.unwrap();
    let sent = s.get(&d.id).await.unwrap();
    assert_eq!(sent.status, DraftStatus::Sent);
    assert_eq!(sent.sent_message_id.as_deref(), Some("sent-1"));
    assert!(service(&s, &f).send(&d.id, sent.revision).await.is_err());
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.kind == "send")
            .count(),
        1
    );
}
#[tokio::test]
async fn offline_send_never_records_an_attempt_and_rejection_retains_editable_content() {
    let s = store().await;
    let offline = Fake {
        offline: true,
        ..Default::default()
    };
    let d = local(&s).await;
    assert!(service(&s, &offline).send(&d.id, d.revision).await.is_err());
    assert!(s.pending(&d.id).await.unwrap().is_none());
    let f = Fake::default();
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    *f.failure.lock().unwrap() = Some("rejected");
    assert!(service(&s, &f).send(&d.id, d.revision).await.is_err());
    assert!(s.get(&d.id).await.unwrap().editable());
    assert_eq!(s.get(&d.id).await.unwrap().fields, d.fields);
}
#[tokio::test]
async fn preflight_changed_clean_remote_never_sends_unreviewed_content() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    {
        let mut r = f.remote.lock().unwrap();
        r[0].message_id = "changed".into();
        r[0].fields.to = "other@example.com".into();
    }
    assert!(matches!(
        service(&s, &f).send(&d.id, d.revision).await,
        Err(AppError::DraftRevisionConflict)
    ));
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn tombstone_and_unknown_delete_survive_reconciliation() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).delete(&d.id, d.revision).await.is_err());
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Deleted);
    assert_eq!(f.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn unsupported_draft_cannot_upload_send_or_be_deleted() {
    let s = store().await;
    let mut r = remote("rich", fields());
    r.issues.push("HTML".into());
    let f = Fake {
        remote: Mutex::new(vec![r]),
        ..Default::default()
    };
    service(&s, &f).sync().await.unwrap();
    let d = s.list(1).await.unwrap().remove(0);
    assert_eq!(d.status, DraftStatus::Unsupported);
    assert!(s.save(&d.id, d.revision, fields()).await.is_err());
    assert!(service(&s, &f).send(&d.id, d.revision).await.is_err());
    assert!(service(&s, &f).delete(&d.id, d.revision).await.is_err());
    assert!(f.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn restart_pending_send_becomes_unknown_without_automatic_send() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    let a = Attempt {
        id: random_id(),
        kind: "send".into(),
        remote_id: d.remote.as_ref().map(|r| r.id.clone()),
        mime: build_mime_message(&d, "me@example.com", "test", true).unwrap(),
        draft: d.clone(),
    };
    s.begin(&a).await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Sending);
    service(&s, &f).sync().await.unwrap();
    let unknown = s.get(&d.id).await.unwrap();
    assert_eq!(unknown.status, DraftStatus::Unknown);
    let editable = service(&s, &f)
        .resolve_send(&a.id, SendResolution::AllowManualRetry)
        .await
        .unwrap();
    assert_eq!(editable.status, DraftStatus::Local);
    assert!(editable.remote.is_none());
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
#[test]
fn address_grammar_and_mime_headers_and_reply_all() {
    let a =
        parse_addresses("\"Doe, Jane\" <jane@example.com>, Friends: a@example.com, b@example.com;")
            .unwrap();
    assert_eq!(a.len(), 3);
    assert_eq!(a[0].0, "Doe, Jane");
    assert!(parse_addresses("person@example.com\r\nBcc: hidden@example.com").is_err());
    assert!(parse_addresses("not an address").is_err());
    let source = Message {
        from_addr: Some("wrong@example.com".into()),
        reply_to: Some("\"Doe, Jane\" <jane@example.com>".into()),
        to_addrs: Some("me@example.com, jane@example.com, other@example.com".into()),
        cc_addrs: Some("other@example.com, cc@example.com".into()),
        bcc_addrs: Some("secret@example.com".into()),
        message_id: Some("<source@example.com>".into()),
        references: Some("<older@example.com>".into()),
        thread_id: Some("thread".into()),
        subject: Some("Hello".into()),
        ..Default::default()
    };
    let mut d =
        Draft::try_compose(1, "me@example.com", ComposeMode::ReplyAll, Some(source)).unwrap();
    assert!(!d.fields.to.contains("me@example.com"));
    assert!(d.fields.to.contains("jane@example.com"));
    assert_eq!(d.fields.cc, "cc@example.com");
    assert!(d.fields.bcc.is_empty());
    d.fields.bcc = "hidden@example.com".into();
    d.fields.subject = "Grüße 世界".into();
    let bytes = build_mime_message(&d, "me@example.com", "operation", true).unwrap();
    let (headers, _) = mailparse::parse_headers(&bytes).unwrap();
    assert_eq!(
        headers
            .iter()
            .find(|h| h.get_key_ref() == "Subject")
            .unwrap()
            .get_value(),
        "Grüße 世界"
    );
    assert_eq!(
        headers
            .iter()
            .find(|h| h.get_key_ref() == "In-Reply-To")
            .unwrap()
            .get_value(),
        "<source@example.com>"
    );
    assert_eq!(
        headers
            .iter()
            .find(|h| h.get_key_ref() == "Bcc")
            .unwrap()
            .get_value(),
        "<hidden@example.com>"
    );
    d.fields.subject = "hello\nInjected: yes".into();
    assert!(build_mime_message(&d, "me@example.com", "operation", true).is_err());
}

#[tokio::test]
async fn save_during_send_remote_creation_does_not_send_newer_content() {
    let s = store().await;
    let d = local(&s).await;
    let f = Fake {
        during_upload: Some(s.clone()),
        ..Default::default()
    };
    assert!(matches!(
        service(&s, &f).send(&d.id, d.revision).await,
        Err(AppError::DraftRevisionConflict)
    ));
    assert!(f.calls.lock().unwrap().iter().all(|a| a.kind != "send"));
    assert_eq!(
        s.get(&d.id).await.unwrap().fields.body_text,
        "new local edit"
    );
}

#[tokio::test]
async fn outside_rich_edit_after_upload_is_conflict_not_a_safe_baseline() {
    let s = store().await;
    let d = local(&s).await;
    let f = Fake {
        outside_upload: true,
        ..Default::default()
    };
    assert!(service(&s, &f).send(&d.id, d.revision).await.is_err());
    let d = s.get(&d.id).await.unwrap();
    assert_eq!(d.status, DraftStatus::Conflict);
    assert_eq!(d.fields.body_text, "hello");
    assert_eq!(
        d.conflict.as_ref().unwrap().issues,
        ["Attachment or inline image"]
    );
    assert!(f.calls.lock().unwrap().iter().all(|a| a.kind != "send"));
    let adopted = service(&s, &f)
        .conflict(&d.id, d.revision, ConflictChoice::UseGmail, None)
        .await
        .unwrap();
    assert_eq!(adopted.status, DraftStatus::Unsupported);
}

#[tokio::test]
async fn file_reopen_preserves_fields_and_pending_send_and_upgrade_preserves_messages() {
    use sqlx::sqlite::SqliteConnectOptions;
    let path = std::env::temp_dir().join(format!("aime-draft-{}.sqlite", random_id()));
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options.clone())
        .await
        .unwrap();
    for source in [
        include_str!("../../../migrations/0001_init_email_accounts.sql"),
        include_str!("../../../migrations/0002_labels.sql"),
        include_str!("../../../migrations/003_gmail_messages.sql"),
        include_str!("../../../migrations/0004_idx_messages.sql"),
    ] {
        sqlx::raw_sql(source).execute(&pool).await.unwrap();
    }
    sqlx::query("INSERT INTO email_accounts(id,email,account_name,account_config) VALUES(1,'me@example.com','Test','{}')").execute(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO messages(account_id,provider_msg_id,subject) VALUES(1,'cached','Existing')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../../../migrations/0005_drafts.sql"))
        .execute(&pool)
        .await
        .unwrap();
    let s = Store(pool.clone());
    let d = local(&s).await;
    let attempt = Attempt {
        id: random_id(),
        kind: "send".into(),
        draft: d.clone(),
        remote_id: Some("remote".into()),
        mime: build_mime_message(&d, "me@example.com", "op", true).unwrap(),
    };
    s.begin(&attempt).await.unwrap();
    drop(s);
    pool.close().await;
    let pool = DbPool::connect_with(options).await.unwrap();
    let s = Store(pool.clone());
    assert_eq!(s.get(&d.id).await.unwrap().fields, d.fields);
    assert_eq!(s.pending(&d.id).await.unwrap().unwrap().id, attempt.id);
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT subject FROM messages WHERE provider_msg_id='cached'"
        )
        .fetch_one(&pool)
        .await
        .unwrap(),
        "Existing"
    );
    pool.close().await;
    std::fs::remove_file(path).unwrap();
}

#[tokio::test]
async fn uncertain_update_then_gmail_edit_preserves_conflict_instead_of_freezing_forever() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    let mut fields = d.fields.clone();
    fields.subject = "Local edit".into();
    s.save(&d.id, d.revision, fields).await.unwrap();
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).sync().await.is_err());
    {
        let mut r = f.remote.lock().unwrap();
        r[0].fields.subject = "Gmail edit after upload".into();
        r[0].message_id = "next".into();
    }
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    assert_eq!(d.status, DraftStatus::Conflict);
    assert_eq!(d.fields.subject, "Local edit");
    assert_eq!(
        d.conflict.unwrap().fields.subject,
        "Gmail edit after upload"
    );
    assert!(s.pending(&d.id).await.unwrap().is_none());
}

#[tokio::test]
async fn acknowledged_base_differing_from_local_fields_is_dirty_without_normalization() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    service(&s, &f).sync().await.unwrap();
    let mut d = s.get(&d.id).await.unwrap();
    let revision = d.revision;
    d.remote.as_mut().unwrap().fields.body_text = "hello\r\n".into();
    s.write(&d, revision).await.unwrap();
    {
        let mut r = f.remote.lock().unwrap();
        r[0].message_id = "new-id".into();
        r[0].fields.subject = "Clean external edit".into();
    }
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    assert_eq!(d.status, DraftStatus::Conflict);
}
#[tokio::test]
async fn both_conflict_choices_keep_external_sender_alias_read_only() {
    for choice in [ConflictChoice::UseGmail, ConflictChoice::KeepLocalAsCopy] {
        let s = store().await;
        let f = Fake::default();
        let d = local(&s).await;
        service(&s, &f).sync().await.unwrap();
        let d = s.get(&d.id).await.unwrap();
        let mut fields = d.fields.clone();
        fields.subject = "Local edit".into();
        s.save(&d.id, d.revision, fields).await.unwrap();
        {
            let mut r = f.remote.lock().unwrap();
            r[0].message_id = "outside".into();
            r[0].from = "alias@example.com".into();
        }
        service(&s, &f).sync().await.unwrap();
        let d = s.get(&d.id).await.unwrap();
        service(&s, &f)
            .conflict(&d.id, d.revision, choice, None)
            .await
            .unwrap();
        assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Unsupported);
    }
}

#[tokio::test]
async fn concurrent_send_calls_submit_only_once() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    let svc = service(&s, &f);
    svc.sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    let (first, second) = tokio::join!(svc.send(&d.id, d.revision), svc.send(&d.id, d.revision));
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
    assert_eq!(
        f.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.kind == "send")
            .count(),
        1
    );
}

#[test]
fn reply_to_own_sent_mail_uses_visible_recipients_without_bcc() {
    let source = Message {
        from_addr: Some("me@example.com".into()),
        to_addrs: Some("recipient@example.com".into()),
        bcc_addrs: Some("hidden@example.com".into()),
        message_id: Some("<sent@example.com>".into()),
        ..Default::default()
    };
    let d = Draft::try_compose(1, "me@example.com", ComposeMode::Reply, Some(source)).unwrap();
    assert_eq!(d.fields.to, "recipient@example.com");
    assert!(d.fields.bcc.is_empty());
}

fn html_source(html: &str) -> Message {
    Message {
        message_id: Some("<source@example.com>".into()),
        from_addr: Some("Acme <info@acme.com>".into()),
        date_header: Some("Mon, 12 Oct 2026".into()),
        to_addrs: Some("me@example.com".into()),
        subject: Some("Offer".into()),
        contents: vec![crate::email::repo::MessageContents {
            provider_msg_id: "m".into(),
            mime_type: "text/html".into(),
            body: html.into(),
        }],
        ..Default::default()
    }
}

#[test]
fn try_compose_new_is_empty_and_missing_source_is_an_error() {
    let d = Draft::try_compose(1, "me@example.com", ComposeMode::New, None).unwrap();
    assert_eq!(d.fields, DraftFields::default());
    for mode in [
        ComposeMode::Reply,
        ComposeMode::ReplyAll,
        ComposeMode::Forward,
    ] {
        assert!(matches!(
            Draft::try_compose(1, "me@example.com", mode, None),
            Err(AppError::DraftNotFound)
        ));
    }
}

#[test]
fn reply_quotes_html_body_verbatim_inside_blockquote_and_nested_quotes_survive() {
    let nested = r#"<table><tr><td style="color:red">Hi</td></tr></table><div class="gmail_quote"><blockquote class="gmail_quote">earlier</blockquote></div>"#;
    let source = html_source(&format!(
        "<html><head><style>p{{color:red}}</style></head><BODY class=x>{nested}</BODY></html>"
    ));
    let d = Draft::try_compose(1, "me@example.com", ComposeMode::Reply, Some(source)).unwrap();
    assert_eq!(d.fields.subject, "Re: Offer");
    assert_eq!(d.fields.body_html, "");
    assert!(d
        .fields
        .quote_html
        .contains(&format!("padding-left:1ex\">{nested}</blockquote>")));
    assert!(d
        .fields
        .quote_html
        .contains("On Mon, 12 Oct 2026, Acme &lt;info@acme.com&gt; wrote:"));
    assert!(!d.fields.quote_html.contains("<style"));
    let (body, quote) = split_gmail_quote(&d.fields.quote_html);
    assert_eq!((body, quote), ("", d.fields.quote_html.as_str()));
}

#[test]
fn reply_to_text_only_source_escapes_it() {
    let mut source = html_source("");
    source.contents[0].mime_type = "text/plain".into();
    source.contents[0].body = "1 < 2 & \"x\"\nline2".into();
    let d = Draft::try_compose(1, "me@example.com", ComposeMode::Reply, Some(source)).unwrap();
    assert!(d
        .fields
        .quote_html
        .contains("1 &lt; 2 &amp; &quot;x&quot;<br>line2"));
}

#[test]
fn forward_has_header_and_no_blockquote() {
    let source = html_source("<p>Deal</p>");
    let d = Draft::try_compose(1, "me@example.com", ComposeMode::Forward, Some(source)).unwrap();
    assert_eq!(d.fields.subject, "Fwd: Offer");
    assert!(d
        .fields
        .quote_html
        .contains("Start of the forwarded message:<br>From: Acme &lt;info@acme.com&gt;<br>"));
    assert!(d.fields.quote_html.ends_with("<br><p>Deal</p></div>"));
    assert!(!d.fields.quote_html.contains("blockquote"));
    assert!(d.in_reply_to.is_none());
}

#[test]
fn split_gmail_quote_round_trips_body_and_quote() {
    let (body, quote) = (
        "<div dir=\"ltr\">hi</div>",
        "<div class=\"gmail_quote gmail_quote_container\">q</div>",
    );
    assert_eq!(split_gmail_quote(&format!("{body}{quote}")), (body, quote));
    assert_eq!(split_gmail_quote(body), (body, ""));
}

#[test]
fn mime_message_is_alternative_with_html_body_plus_quote_and_text_body() {
    let mut d = Draft::new(1, fields());
    d.fields.body_html = "<b>hello</b>".into();
    d.fields.quote_html = "<div class=\"gmail_quote\">q</div>".into();
    let bytes = build_mime_message(&d, "me@example.com", "op", false).unwrap();
    let parsed = mailparse::parse_mail(&bytes).unwrap();
    assert_eq!(parsed.ctype.mimetype, "multipart/alternative");
    let bodies: Vec<_> = parsed
        .subparts
        .iter()
        .map(|p| (p.ctype.mimetype.clone(), p.get_body().unwrap()))
        .collect();
    assert_eq!(bodies[0], ("text/plain".into(), "hello".into()));
    assert_eq!(bodies[1].0, "text/html");
    assert!(bodies[1]
        .1
        .contains("<b>hello</b><div class=\"gmail_quote\">q</div>"));
}

#[tokio::test]
async fn unknown_create_with_matching_operation_but_edited_text_is_conflict() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).sync().await.is_err());
    f.remote.lock().unwrap()[0].fields.body_text = "edited in Gmail".into();
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Conflict);
}

#[tokio::test]
async fn unknown_create_tolerates_crlf_and_trailing_newline() {
    let s = store().await;
    let f = Fake::default();
    let d = local(&s).await;
    *f.failure.lock().unwrap() = Some("unknown-after");
    assert!(service(&s, &f).sync().await.is_err());
    f.remote.lock().unwrap()[0].fields.body_text = "hello\r\n".into();
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
}

#[tokio::test]
async fn gmail_edit_after_crlf_round_trip_is_adopted_not_a_conflict() {
    let s = store().await;
    let f = Fake {
        wire_crlf: true,
        ..Default::default()
    };
    let mut d = Draft::new(1, fields());
    d.fields.body_text = "line one\nline two".into();
    s.insert(&d).await.unwrap();
    service(&s, &f).sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
    {
        let mut r = f.remote.lock().unwrap();
        r[0].message_id = "gmail-edit".into();
        r[0].fields.subject = "Edited in Gmail".into();
    }
    service(&s, &f).sync().await.unwrap();
    let d = s.get(&d.id).await.unwrap();
    assert_eq!(d.status, DraftStatus::Synced);
    assert_eq!(d.fields.subject, "Edited in Gmail");
}

#[tokio::test]
async fn untouched_draft_with_only_a_quote_is_not_uploaded_until_text_is_typed() {
    let s = store().await;
    let f = Fake::default();
    let svc = service(&s, &f);
    let d = Draft::new(
        1,
        DraftFields {
            to: "you@example.com".into(),
            quote_html: "<div class=\"gmail_quote\">original</div>".into(),
            ..Default::default()
        },
    );
    s.insert(&d).await.unwrap();
    svc.sync().await.unwrap();
    assert!(f.calls.lock().unwrap().is_empty());
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Local);

    let mut typed = d.fields.clone();
    typed.body_text = "Thanks".into();
    typed.body_html = "<div>Thanks</div>".into();
    s.save(&d.id, d.revision, typed).await.unwrap();
    svc.sync().await.unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 1);
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
}

#[tokio::test]
async fn uploaded_draft_cleared_to_empty_still_syncs() {
    let s = store().await;
    let f = Fake::default();
    let svc = service(&s, &f);
    let d = local(&s).await;
    svc.sync().await.unwrap();
    let synced = s.get(&d.id).await.unwrap();
    let mut cleared = synced.fields.clone();
    cleared.body_text = String::new();
    cleared.body_html = "<div><br></div>".into();
    s.save(&d.id, synced.revision, cleared).await.unwrap();
    svc.sync().await.unwrap();
    assert_eq!(f.calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn rejected_draft_is_not_rechecked_until_edited() {
    let s = store().await;
    let f = Fake::default();
    let svc = service(&s, &f);
    let d = local(&s).await;
    let mut bad = d.fields.clone();
    bad.to = "you@example.com asdf".into();
    let saved = s.save(&d.id, d.revision, bad).await.unwrap();
    svc.sync().await.unwrap();
    let rejected = s.get(&d.id).await.unwrap();
    assert_eq!(rejected.status, DraftStatus::Local);
    assert_eq!(rejected.issues.len(), 1);
    assert_eq!(rejected.revision, saved.revision + 1);
    svc.sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().revision, rejected.revision);

    let fixed = s.save(&d.id, rejected.revision, fields()).await.unwrap();
    assert!(fixed.issues.is_empty());
    svc.sync().await.unwrap();
    assert_eq!(s.get(&d.id).await.unwrap().status, DraftStatus::Synced);
    assert_eq!(f.calls.lock().unwrap().len(), 1);
}
