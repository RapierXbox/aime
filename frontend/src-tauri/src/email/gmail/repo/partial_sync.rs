use google_gmail1::api::{
    History, HistoryLabelAdded, HistoryLabelRemoved, HistoryMessageAdded, HistoryMessageDeleted,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{Connection, Executor, Sqlite, Transaction};
use tauri::ipc::Channel;

use crate::{AppError, Progress, ProgressReporter};

impl super::GmailRepo {
    pub async fn apply_history(
        &self,
        history: Vec<History>,
        updates: Channel<Progress>,
    ) -> Result<(), AppError> {
        // TODO: error handling: this probably shouldnt propagate errors
        let mut tx = self.db_pool.begin().await?;
        let len = history.len() as u64;

        for (i, h) in history.into_iter().enumerate() {
            updates.report(Progress::Update {
                completed: i as u64 + 1,
                out_of: Some(len),
            });

            if let History {
                labels_added: Some(added),
                ..
            } = h
            {
                self.apply_history_labels_added(&mut tx, added).await?;
            }

            if let History {
                messages_added: Some(added),
                ..
            } = h
            {
                self.apply_history_messages_added(&mut tx, added).await?;
            }

            if let History {
                messages_deleted: Some(del),
                ..
            } = h
            {
                self.apply_history_messages_deleted(&mut tx, del).await?;
            }

            if let History {
                labels_removed: Some(rem),
                ..
            } = h
            {
                self.apply_history_labels_removed(&mut tx, rem).await?;
            }
        }

        tx.commit().await?;
        Ok(())
    }

    async fn apply_history_labels_removed(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        rem: Vec<HistoryLabelRemoved>,
    ) -> Result<(), AppError> {
        Ok(for msg in rem {
            let HistoryLabelRemoved {
                label_ids: Some(label_ids),
                message:
                    Some(google_gmail1::api::Message {
                        id: Some(msg_id), ..
                    }),
            } = msg
            else {
                info!(
                    "in partial sync in message added missing field (req: id, thread_id): {msg:?}"
                );
                continue;
            };

            for label_id in label_ids {
                sqlx::query!(
                "DELETE FROM message_has_label WHERE account_id = ? AND provider_msg_id = ? AND label_id = ?",
                self.account_id,
                msg_id,
                label_id
             )
            .execute(&mut **tx)
            .await?;
            }
        })
    }

    async fn apply_history_labels_added(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        added: Vec<HistoryLabelAdded>,
    ) -> Result<(), AppError> {
        Ok(for l in added {
            let HistoryLabelAdded {
                label_ids: Some(ids),
                message:
                    Some(google_gmail1::api::Message {
                        id: Some(msg_id), ..
                    }),
            } = l
            else {
                info!("in partial sync in labels added: missing fields {l:?}");
                continue;
            };

            for label_id in ids {
                sqlx::query!(
                "INSERT INTO message_has_label (account_id, provider_msg_id, label_id) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                self.account_id,
                msg_id,
                label_id
            ).execute(&mut **tx).await?;
            }
        })
    }

    async fn apply_history_messages_added(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        added: Vec<HistoryMessageAdded>,
    ) -> Result<(), AppError> {
        Ok(for msg in added {
            let HistoryMessageAdded {
                message:
                    Some(google_gmail1::api::Message {
                        id: Some(msg_id),
                        thread_id: Some(thread_id),
                        ..
                    }),
            } = msg
            else {
                info!(
                    "in partial sync in message added missing field (req: id, thread_id): {msg:?}"
                );
                continue;
            };

            sqlx::query!(
                "INSERT INTO messages (account_id, provider_msg_id, thread_id) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                self.account_id,
                msg_id,
                thread_id
            )
            .execute(&mut **tx)
            .await?;
        })
    }

    async fn apply_history_messages_deleted(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        del: Vec<HistoryMessageDeleted>,
    ) -> Result<(), AppError> {
        Ok(for msg in del {
            let HistoryMessageDeleted {
                message:
                    Some(google_gmail1::api::Message {
                        id: Some(msg_id), ..
                    }),
            } = msg
            else {
                info!("in partial sync in message added missing field (req: id): {msg:?}");
                continue;
            };

            sqlx::query!(
                "DELETE FROM messages WHERE account_id = ? AND provider_msg_id = ?",
                self.account_id,
                msg_id,
            )
            .execute(&mut **tx)
            .await?;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_history_labels_added() {}

    #[test]
    fn apply_history_labels_removed() {}

    #[test]
    fn apply_history_messages_added() {}

    #[test]
    fn apply_history_messages_deleted() {}

    #[test]
    fn apply_history_skips_entry_missing_required_fields() {}
}
