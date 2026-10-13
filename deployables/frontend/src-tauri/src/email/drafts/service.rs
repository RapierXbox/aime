use super::{
    model::*,
    store::{Attempt, Store},
};
use crate::{email::repo::Message, AppError};
use async_trait::async_trait;

#[derive(Debug)]
pub enum ProviderError {
    Rejected,
    Authorization,
    Unknown,
}
#[derive(Debug)]
pub struct Receipt {
    pub remote: Option<RemoteDraft>,
    pub sent_id: Option<String>,
    /// Message id from the create/update response; `remote` is ours only if it carries this id.
    pub uploaded_id: Option<String>,
}
#[async_trait]
pub trait Provider: Send + Sync {
    async fn list(&self) -> Result<Vec<RemoteDraft>, ProviderError>;
    async fn get(&self, id: &str) -> Result<Option<RemoteDraft>, ProviderError>;
    async fn mutate(&self, attempt: &Attempt) -> Result<Receipt, ProviderError>;
    async fn find_sent(&self, operation: &str) -> Result<Option<String>, ProviderError>;
    async fn source(&self, id: &str) -> Result<Message, ProviderError>;
    async fn ready(&self) -> Result<(), ProviderError>;
}
impl From<ProviderError> for AppError {
    fn from(value: ProviderError) -> Self {
        match value {
            ProviderError::Authorization => AppError::EmailAuthorizationRequired,
            ProviderError::Rejected => AppError::EmailProviderRejected,
            ProviderError::Unknown => AppError::EmailNetworkError,
        }
    }
}
pub struct Service<'a> {
    pub store: Store,
    pub provider: &'a dyn Provider,
    pub sender: &'a str,
    pub account: i64,
}
/// Whether two drafts agree on subject and text body, ignoring CRLF and trailing newlines.
// ponytail: only for the unknown-outcome path, where no response id exists; drop once Gmail is
// verified to keep X-Aime-Operation across web edits, so the header alone proves our upload
fn same_subject_and_text(a: &DraftFields, b: &DraftFields) -> bool {
    let text = |f: &DraftFields| {
        f.body_text
            .replace("\r\n", "\n")
            .trim_end_matches('\n')
            .to_string()
    };
    a.subject == b.subject && text(a) == text(b)
}

impl Service<'_> {
    fn adopt(&self, draft: &mut Draft, remote: RemoteDraft) {
        draft.adopt(remote);
        if draft
            .remote
            .as_ref()
            .is_some_and(|r| !r.from.eq_ignore_ascii_case(self.sender))
        {
            draft.issues.push("Sender alias is not supported".into());
            draft.status = DraftStatus::Unsupported;
        }
    }

    pub async fn create(
        &self,
        mode: ComposeMode,
        source_id: Option<String>,
    ) -> Result<Draft, AppError> {
        let source = match source_id {
            Some(id) => Some(self.provider.source(&id).await?),
            None => None,
        };
        let draft = Draft::try_compose(self.account, self.sender, mode, source)?;
        self.store.insert(&draft).await?;
        Ok(draft)
    }
    async fn reconcile_attempt(&self, a: &Attempt, all: &[RemoteDraft]) -> Result<(), AppError> {
        if a.kind == "send" {
            if let Some(sent) = self.provider.find_sent(&a.id).await? {
                self.store.finish(a, "confirmed", None, Some(sent)).await?;
            } else {
                self.store.finish(a, "unknown", None, None).await?;
            }
        } else if a.kind == "delete" {
            if self
                .provider
                .get(a.remote_id.as_deref().ok_or(AppError::DraftLocked)?)
                .await?
                .is_none()
            {
                self.store.finish(a, "confirmed", None, None).await?;
            } else {
                self.store.finish(a, "unknown", None, None).await?;
            }
        } else {
            let remote = if a.kind == "create" {
                let matches: Vec<_> = all
                    .iter()
                    .filter(|r| r.operation_id.as_deref() == Some(&a.id))
                    .collect();
                if matches.len() == 1 {
                    Some(matches[0].clone())
                } else {
                    None
                }
            } else {
                self.provider
                    .get(a.remote_id.as_deref().ok_or(AppError::DraftLocked)?)
                    .await?
            };
            let remote = remote.map(|mut r| {
                if !r.from.eq_ignore_ascii_case(self.sender) {
                    r.issues.push("Sender alias is not supported".into());
                }
                r
            });
            match remote {
                Some(r)
                    if r.operation_id.as_deref() == Some(&a.id)
                        && same_subject_and_text(&r.fields, &a.draft.fields)
                        && same_threading_headers(&a.draft, &r)
                        && r.issues.is_empty() =>
                {
                    self.store.finish(a, "confirmed", Some(r), None).await?;
                }
                Some(r)
                    if (a.kind == "create" && r.operation_id.as_deref() == Some(&a.id))
                        || a.draft
                            .remote
                            .as_ref()
                            .is_some_and(|base| base.message_id != r.message_id) =>
                {
                    // Preserve a subsequent Gmail edit rather than silently establish it as the uploaded baseline.
                    self.store.finish(a, "resolved", Some(r), None).await?;
                }
                None if a.kind == "update" => {
                    let mut d = self.store.finish(a, "resolved", None, None).await?;
                    let revision = d.revision;
                    d.status = DraftStatus::Missing;
                    d.revision += 1;
                    self.store.write(&d, revision).await?;
                }
                _ => {
                    self.store.finish(a, "unknown", None, None).await?;
                }
            }
        }
        Ok(())
    }
    // Caller holds GmailClient.draft_lock. A complete remote scan precedes every deletion inference.
    pub async fn sync(&self) -> Result<(), AppError> {
        let all = self.provider.list().await?;
        let locals = self.store.list(self.account).await?;
        let pending: Vec<_> =
            futures::future::try_join_all(locals.iter().map(|d| self.store.pending(&d.id))).await?;
        for a in pending.iter().flatten() {
            self.reconcile_attempt(a, &all).await?;
        }
        let locals = self.store.list(self.account).await?;
        for r in &all {
            if locals
                .iter()
                .any(|d| d.remote.as_ref().is_some_and(|base| base.id == r.id))
            {
                continue;
            }
            // An unknown create must not appear as a second locally imported editable draft.
            if pending
                .iter()
                .flatten()
                .any(|a| a.kind == "create" && r.operation_id.as_deref() == Some(&a.id))
            {
                continue;
            }
            let mut d = Draft::new(self.account, r.fields.clone());
            self.adopt(&mut d, r.clone());
            self.store.insert(&d).await?;
        }
        for d in locals {
            if matches!(
                d.status,
                DraftStatus::Sent
                    | DraftStatus::Unsupported
                    | DraftStatus::Conflict
                    | DraftStatus::Missing
                    | DraftStatus::Unknown
                    | DraftStatus::Sending
            ) {
                // Unsupported drafts can become editable through a Gmail edit; refresh without writing them remotely.
                if d.status == DraftStatus::Unsupported {
                    self.refresh_scanned(d, &all).await?;
                }
                continue;
            }
            if d.status == DraftStatus::Deleted {
                if self.store.pending(&d.id).await?.is_none() && d.remote.is_some() {
                    self.delete_remote(d).await?;
                }
                continue;
            }
            let mut d = self.refresh_scanned(d, &all).await?;
            // A non-empty `issues` on a Local draft means it was already rejected at this
            // revision; `Store::save` clears it, so it's re-checked only after an edit.
            if d.status == DraftStatus::Local && !d.is_untouched() && d.issues.is_empty() {
                // Invalid or unfinished recipients remain locally durable; no automatic destructive conversion.
                match build_mime_message(&d, self.sender, "validation", false) {
                    Ok(_) => {
                        let d = self.refresh(d).await?;
                        if d.status == DraftStatus::Local {
                            self.mutation(d, "upload").await?;
                        }
                    }
                    Err(e) => {
                        log::info!("draft {} kept local, not uploaded: {e}", d.id);
                        let expected = d.revision;
                        d.issues = vec![e.to_string()];
                        d.revision += 1;
                        self.store.write(&d, expected).await?;
                    }
                }
            }
        }
        Ok(())
    }
    async fn refresh_scanned(&self, d: Draft, all: &[RemoteDraft]) -> Result<Draft, AppError> {
        let Some(base) = d.remote.as_ref() else {
            return Ok(d);
        };
        let remote = match all.iter().find(|r| r.id == base.id) {
            Some(r) => Some(r.clone()),
            None => self.provider.get(&base.id).await?,
        };
        self.apply_remote(d, remote).await
    }
    pub async fn refresh(&self, d: Draft) -> Result<Draft, AppError> {
        let Some(base) = d.remote.as_ref() else {
            return Ok(d);
        };
        let remote = self.provider.get(&base.id).await?;
        self.apply_remote(d, remote).await
    }
    async fn apply_remote(
        &self,
        mut d: Draft,
        remote: Option<RemoteDraft>,
    ) -> Result<Draft, AppError> {
        let Some(base) = d.remote.clone() else {
            return Ok(d);
        };
        let expected = d.revision;
        match remote {
            Some(r) if r.message_id != base.message_id => {
                if d.revision != d.acknowledged_revision || d.fields != base.fields {
                    d.conflict = Some(r);
                    d.status = DraftStatus::Conflict;
                    d.revision += 1;
                } else {
                    self.adopt(&mut d, r);
                }
                self.store.write(&d, expected).await?;
            }
            None => {
                d.status = DraftStatus::Missing;
                d.revision += 1;
                self.store.write(&d, expected).await?;
            }
            _ => {}
        }
        Ok(d)
    }
    pub async fn mutation(&self, d: Draft, kind: &str) -> Result<Draft, AppError> {
        self.provider.ready().await?;
        let kind = if kind == "upload" {
            if d.remote.is_some() {
                "update"
            } else {
                "create"
            }
        } else {
            kind
        };
        let id = random_id();
        let bytes = if kind == "delete" {
            vec![]
        } else {
            build_mime_message(&d, self.sender, &id, kind == "send")?
        };
        let a = Attempt {
            id,
            kind: kind.into(),
            remote_id: d.remote.as_ref().map(|r| r.id.clone()),
            draft: d,
            mime: bytes,
        };
        self.store.begin(&a).await?;
        match self.provider.mutate(&a).await {
            Ok(mut r) => {
                if let Some(remote) = &mut r.remote {
                    if !remote.from.eq_ignore_ascii_case(self.sender) {
                        remote.issues.push("Sender alias is not supported".into());
                    }
                }
                // Gmail gives every draft version a new message id; any other id is a Gmail-side edit.
                let ours = r
                    .remote
                    .as_ref()
                    .is_none_or(|x| r.uploaded_id.as_deref() == Some(&x.message_id));
                let outcome = if ours { "confirmed" } else { "resolved" };
                self.store.finish(&a, outcome, r.remote, r.sent_id).await
            }
            Err(e) => {
                let uncertain = matches!(e, ProviderError::Unknown);
                self.store
                    .finish(
                        &a,
                        if uncertain { "unknown" } else { "rejected" },
                        None,
                        None,
                    )
                    .await?;
                Err(if uncertain {
                    AppError::EmailOutcomeUnknown
                } else {
                    e.into()
                })
            }
        }
    }
    pub async fn send(&self, id: &str, revision: u32) -> Result<Draft, AppError> {
        let mut d = self.store.get(id).await?;
        if d.revision != revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !matches!(d.status, DraftStatus::Local | DraftStatus::Synced)
            || self.store.pending(id).await?.is_some()
        {
            return Err(AppError::DraftLocked);
        }
        build_mime_message(&d, self.sender, "validation", true)?;
        d = self.refresh(d).await?;
        if d.revision != revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !matches!(d.status, DraftStatus::Local | DraftStatus::Synced) {
            return Err(AppError::DraftLocked);
        }
        // Persist a remote draft first. A failed create never becomes a queued send.
        if d.remote.is_none() {
            let reviewed_revision = d.revision;
            let reviewed_fields = d.fields.clone();
            d = self.mutation(d, "upload").await?;
            // Creating a remote draft is editable. A save during that request must never become an unreviewed send.
            if d.revision != reviewed_revision + 2 || d.fields != reviewed_fields {
                return Err(AppError::DraftRevisionConflict);
            }
            if !matches!(d.status, DraftStatus::Local | DraftStatus::Synced) {
                return Err(AppError::DraftLocked);
            }
        }
        self.mutation(d, "send").await
    }
    pub async fn conflict(
        &self,
        id: &str,
        revision: u32,
        choice: ConflictChoice,
        local_fields: Option<DraftFields>,
    ) -> Result<Draft, AppError> {
        let mut d = self.store.get(id).await?;
        if d.revision != revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !d.editable() {
            return Err(AppError::DraftLocked);
        }
        match choice {
            ConflictChoice::UseGmail => {
                let remote = d.conflict.clone().ok_or(AppError::DraftLocked)?;
                self.adopt(&mut d, remote);
                self.store.write(&d, revision).await?;
                Ok(d)
            }
            ConflictChoice::KeepLocalAsCopy => {
                let mut copy = Draft::new(
                    self.account,
                    local_fields.unwrap_or_else(|| d.fields.clone()),
                );
                copy.thread_id = d.thread_id.clone();
                copy.in_reply_to = d.in_reply_to.clone();
                copy.references = d.references.clone();
                if let Some(r) = d.conflict.clone() {
                    self.adopt(&mut d, r);
                } else if d.status == DraftStatus::Missing {
                    d.status = DraftStatus::Deleted;
                    d.remote = None;
                    d.revision += 1;
                } else {
                    d.revision += 1;
                    if d.status == DraftStatus::Synced {
                        d.acknowledged_revision = d.revision;
                    }
                }
                self.store.copy_and_write(&copy, &d, revision).await?;
                Ok(copy)
            }
        }
    }
    pub async fn delete(&self, id: &str, revision: u32) -> Result<(), AppError> {
        let mut d = self.store.get(id).await?;
        if d.revision != revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !d.editable() || self.store.pending(id).await?.is_some() {
            return Err(AppError::DraftLocked);
        }
        d.status = DraftStatus::Deleted;
        d.revision += 1;
        self.store.write(&d, revision).await?;
        // Tombstone is durable before the request and retained if offline.
        if d.remote.is_some() {
            self.delete_remote(d).await?;
        }
        Ok(())
    }
    async fn delete_remote(&self, d: Draft) -> Result<(), AppError> {
        let mut d = self.refresh(d).await?;
        if d.status == DraftStatus::Missing {
            let revision = d.revision;
            d.status = DraftStatus::Deleted;
            d.remote = None;
            d.revision += 1;
            self.store.write(&d, revision).await?;
        } else if d.status == DraftStatus::Deleted {
            self.mutation(d, "delete").await?;
        } else {
            return Err(AppError::DraftRevisionConflict);
        }
        Ok(())
    }
    pub async fn resolve_send(
        &self,
        attempt_id: &str,
        resolution: SendResolution,
    ) -> Result<Draft, AppError> {
        let a = self.store.attempt(attempt_id).await?;
        let d = self.store.get(&a.draft.id).await?;
        if a.kind != "send" || d.status != DraftStatus::Unknown {
            return Err(AppError::DraftLocked);
        }
        match resolution {
            SendResolution::ConfirmSent => self.store.finish(&a, "confirmed", None, None).await,
            SendResolution::AllowManualRetry => {
                let mut d = self.store.finish(&a, "resolved", None, None).await?;
                let rev = d.revision;
                d.remote = None;
                d.acknowledged_revision = 0;
                d.revision += 1;
                self.store.write(&d, rev).await?;
                Ok(d)
            }
        }
    }
}
