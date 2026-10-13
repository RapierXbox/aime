use super::model::*;
use crate::{AppError, DbPool};
use serde::{Deserialize, Serialize};
use sqlx::Row;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub kind: String,
    pub draft: Draft,
    pub mime: Vec<u8>,
    pub remote_id: Option<String>,
}
#[derive(Clone)]
pub struct Store(pub DbPool);
impl Store {
    pub async fn get(&self, id: &str) -> Result<Draft, AppError> {
        let data: String = sqlx::query_scalar("SELECT data FROM drafts WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.0)
            .await?
            .ok_or(AppError::DraftNotFound)?;
        Ok(serde_json::from_str(&data)?)
    }
    pub async fn list(&self, account: i64) -> Result<Vec<Draft>, AppError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT data FROM drafts WHERE account_id = ? ORDER BY updated_at DESC, id",
        )
        .bind(account)
        .fetch_all(&self.0)
        .await?;
        rows.into_iter()
            .map(|r| Ok(serde_json::from_str(&r)?))
            .collect()
    }
    pub async fn insert(&self, d: &Draft) -> Result<(), AppError> {
        sqlx::query("INSERT INTO drafts(id,account_id,gmail_draft_id,data,revision,updated_at) VALUES(?,?,?,?,?,?)")
            .bind(&d.id).bind(d.account_id.parse::<i64>().map_err(|_| AppError::ParseAccountID)?)
            .bind(d.remote.as_ref().map(|r| &r.id)).bind(serde_json::to_string(d)?)
            .bind(d.revision).bind(chrono::Utc::now().timestamp_millis()).execute(&self.0).await?;
        Ok(())
    }
    pub async fn copy_and_write(
        &self,
        copy: &Draft,
        original: &Draft,
        expected: u32,
    ) -> Result<(), AppError> {
        let mut tx = self.0.begin().await?;
        let rows = sqlx::query("UPDATE drafts SET gmail_draft_id=?,data=?,revision=?,updated_at=? WHERE id=? AND revision=?")
            .bind(original.remote.as_ref().map(|r| &r.id)).bind(serde_json::to_string(original)?)
            .bind(original.revision).bind(chrono::Utc::now().timestamp_millis()).bind(&original.id).bind(expected)
            .execute(&mut *tx).await?.rows_affected();
        if rows != 1 {
            return Err(AppError::DraftRevisionConflict);
        }
        sqlx::query("INSERT INTO drafts(id,account_id,gmail_draft_id,data,revision,updated_at) VALUES(?,?,NULL,?,?,?)")
            .bind(&copy.id).bind(copy.account_id.parse::<i64>().map_err(|_| AppError::ParseAccountID)?)
            .bind(serde_json::to_string(copy)?).bind(copy.revision).bind(chrono::Utc::now().timestamp_millis())
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }
    pub async fn write(&self, d: &Draft, expected: u32) -> Result<(), AppError> {
        let n = sqlx::query("UPDATE drafts SET gmail_draft_id=?,data=?,revision=?,updated_at=? WHERE id=? AND revision=?")
            .bind(d.remote.as_ref().map(|r| &r.id)).bind(serde_json::to_string(d)?)
            .bind(d.revision).bind(chrono::Utc::now().timestamp_millis()).bind(&d.id).bind(expected)
            .execute(&self.0).await?.rows_affected();
        if n != 1 {
            return Err(AppError::DraftRevisionConflict);
        }
        Ok(())
    }
    pub async fn save(
        &self,
        id: &str,
        revision: u32,
        fields: DraftFields,
    ) -> Result<Draft, AppError> {
        let mut d = self.get(id).await?;
        if d.revision != revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !d.editable() {
            return Err(AppError::DraftLocked);
        }
        d.fields = fields;
        // An edit may fix whatever kept the draft from uploading; sync re-checks it.
        d.issues.clear();
        d.revision += 1;
        if d.status == DraftStatus::Synced {
            d.status = DraftStatus::Local;
        }
        self.write(&d, revision).await?;
        Ok(d)
    }
    pub async fn recover_pending(&self) -> Result<(), AppError> {
        let rows: Vec<String> =
            sqlx::query_scalar("SELECT data FROM draft_attempts WHERE outcome='pending'")
                .fetch_all(&self.0)
                .await?;
        for data in rows {
            let a: Attempt = serde_json::from_str(&data)?;
            self.finish(&a, "unknown", None, None).await?;
        }
        Ok(())
    }
    pub async fn pending(&self, id: &str) -> Result<Option<Attempt>, AppError> {
        let data: Option<String> = sqlx::query_scalar(
            "SELECT data FROM draft_attempts WHERE draft_id=? AND outcome IN ('pending','unknown')",
        )
        .bind(id)
        .fetch_optional(&self.0)
        .await?;
        Ok(data.map(|s| serde_json::from_str(&s)).transpose()?)
    }
    pub async fn attempt(&self, id: &str) -> Result<Attempt, AppError> {
        let data: String = sqlx::query_scalar(
            "SELECT data FROM draft_attempts WHERE id=? AND outcome IN ('pending','unknown')",
        )
        .bind(id)
        .fetch_optional(&self.0)
        .await?
        .ok_or(AppError::DraftLocked)?;
        Ok(serde_json::from_str(&data)?)
    }
    pub async fn begin(&self, a: &Attempt) -> Result<(), AppError> {
        let mut tx = self.0.begin().await?;
        let mut d: Draft = serde_json::from_str(
            &sqlx::query("SELECT data FROM drafts WHERE id=?")
                .bind(&a.draft.id)
                .fetch_one(&mut *tx)
                .await?
                .get::<String, _>("data"),
        )?;
        if d.revision != a.draft.revision {
            return Err(AppError::DraftRevisionConflict);
        }
        if !d.editable() && d.status != DraftStatus::Deleted {
            return Err(AppError::DraftLocked);
        }
        d.attempt_id = Some(a.id.clone());
        d.attempt_kind = Some(a.kind.clone());
        if a.kind == "send" {
            d.status = DraftStatus::Sending;
        }
        // Changing the revision freezes send/delete even when a concurrent save already read the old state.
        d.revision += 1;
        sqlx::query(
            "INSERT INTO draft_attempts(id,draft_id,kind,data,outcome) VALUES(?,?,?,?,'pending')",
        )
        .bind(&a.id)
        .bind(&d.id)
        .bind(&a.kind)
        .bind(serde_json::to_string(a)?)
        .execute(&mut *tx)
        .await?;
        let rows = sqlx::query("UPDATE drafts SET data=?,revision=? WHERE id=? AND revision=?")
            .bind(serde_json::to_string(&d)?)
            .bind(d.revision)
            .bind(&d.id)
            .bind(a.draft.revision)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if rows != 1 {
            return Err(AppError::DraftRevisionConflict);
        }
        tx.commit().await?;
        Ok(())
    }
    // Result and local receipt are committed together; a cache hydration failure is unrelated.
    pub async fn finish(
        &self,
        a: &Attempt,
        outcome: &str,
        remote: Option<RemoteDraft>,
        sent: Option<String>,
    ) -> Result<Draft, AppError> {
        let mut tx = self.0.begin().await?;
        let data: String = sqlx::query_scalar("SELECT data FROM drafts WHERE id=?")
            .bind(&a.draft.id)
            .fetch_one(&mut *tx)
            .await?;
        let mut d: Draft = serde_json::from_str(&data)?;
        d.revision += 1;
        match outcome {
            "unknown" => d.status = DraftStatus::Unknown,
            "confirmed" | "resolved" if outcome == "confirmed" || remote.is_some() => {
                d.attempt_id = None;
                d.attempt_kind = None;
                if a.kind == "send" {
                    d.status = DraftStatus::Sent;
                    d.sent_message_id = sent;
                } else if a.kind == "delete" {
                    d.status = DraftStatus::Deleted;
                    d.remote = None;
                } else {
                    // begin increments once, so an otherwise unchanged draft is at submitted revision + 1.
                    let changed = d.fields != a.draft.fields;
                    if let Some(r) = remote {
                        // "resolved" means the caller found the remote is not our upload.
                        if outcome == "resolved"
                            || r.operation_id.as_deref() != Some(&a.id)
                            || !same_threading_headers(&a.draft, &r)
                            || !r.issues.is_empty()
                        {
                            if d.remote.is_none() {
                                d.remote = Some(r.clone());
                            }
                            d.conflict = Some(r);
                            d.status = DraftStatus::Conflict;
                        } else {
                            // The base is what we uploaded: Gmail's copy differs only by MIME
                            // encoding (CRLF), which would turn the next Gmail edit into a false conflict.
                            d.remote = Some(RemoteDraft {
                                fields: a.draft.fields.clone(),
                                ..r
                            });
                            d.acknowledged_revision = if changed {
                                a.draft.revision
                            } else {
                                d.revision
                            };
                            d.status = if changed {
                                DraftStatus::Local
                            } else {
                                DraftStatus::Synced
                            };
                        }
                    } else {
                        return Err(AppError::EmailOutcomeUnknown);
                    }
                }
            }
            _ => {
                d.attempt_id = None;
                d.attempt_kind = None;
                d.status = if a.kind == "delete" {
                    DraftStatus::Deleted
                } else {
                    DraftStatus::Local
                };
            }
        }
        sqlx::query("UPDATE draft_attempts SET outcome=? WHERE id=?")
            .bind(outcome)
            .bind(&a.id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE drafts SET data=?,revision=?,gmail_draft_id=? WHERE id=?")
            .bind(serde_json::to_string(&d)?)
            .bind(d.revision)
            .bind(d.remote.as_ref().map(|r| &r.id))
            .bind(&d.id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(d)
    }
}
