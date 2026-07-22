use std::{marker::PhantomData, process::Output};

use futures::StreamExt;
use google_gmail1::api::{
    History, HistoryLabelAdded, HistoryLabelRemoved, HistoryMessageAdded, HistoryMessageDeleted,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{Connection, Executor, Sqlite, Transaction};

use crate::{
    email::{
        gmail::GmailError,
        repo::{
            AccountConfig, AddLabelStatus, HistoryID, Label, Message, MessageContents,
            MessageSkeleton, MissingField,
        },
    },
    AppError, DbPool,
};

#[derive(Debug)]
// TODO: add a with_transaction(|tx| {...})
pub struct GmailRepo {
    db_pool: DbPool,
    account_id: i64,
}

mod partial_sync;

impl GmailRepo {
    pub fn new(db_pool: DbPool, account_id: i64) -> GmailRepo {
        Self {
            db_pool,
            account_id,
        }
    }

    pub async fn add_label(&self, label: Label) -> Result<AddLabelStatus, crate::AppError> {
        sqlx::query!(
            "INSERT INTO labels (account_id, id, name, message_list_visibility, label_list_visibility, type)
                VALUES (?, ?, ?, ?, ?, ?)
                ON CONFLICT (account_id, id) DO NOTHING
            ",
            self.account_id,
            label.id,
            label.name,
            label.message_list_visibility,
            label.label_list_visibility,
            label.type_
        )
        .execute(&self.db_pool)
        .await
        .map(|r| match r.rows_affected() {
            0 => AddLabelStatus::AlreadyExists,
            1 => AddLabelStatus::Inserted,
            _ => unreachable!(),
        })
        .map_err(|e| {
            error!("in add_label: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })
    }

    /// Insert message skeletons
    pub async fn insert_skeleton_messages(
        &self,
        msgs: &[google_gmail1::api::Message],
    ) -> Result<(), AppError> {
        let mut tx = self.db_pool.begin().await?;

        for msg in msgs {
            match msg {
                google_gmail1::api::Message {
                    id: Some(msg_id),
                    thread_id: Some(thread_id),
                    ..
                } => {
                    let _ = sqlx::query!(
                    // TODO update labels?
                        "INSERT INTO messages (account_id, provider_msg_id, thread_id) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                        self.account_id,
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

    pub fn stream_all_messages(
        &self,
    ) -> impl futures::Stream<Item = Result<MessageSkeleton, AppError>> + use<'_> {
        sqlx::query!(
            r#"SELECT provider_msg_id, internal_date IS NULL as "previously_cached!: bool" FROM messages WHERE account_id = ?"#,
            self.account_id
        )
        .fetch(&self.db_pool)
        .map(|it| {
            let r = it?;
            Ok(MessageSkeleton {
                provider_msg_id: r.provider_msg_id,
                previously_cached: r.previously_cached,
            })
        })
    }

    pub fn stream_message_skeletons(
        &self,
    ) -> impl futures::Stream<Item = Result<MessageSkeleton, AppError>> + use<'_> {
        sqlx::query!(
            r#"SELECT provider_msg_id, internal_date IS NULL as "previously_cached!: bool"
            FROM messages
            WHERE internal_date IS NULL
                AND account_id = ?
            "#,
            self.account_id
        )
        .fetch(&self.db_pool)
        .map(|it| {
            let r = it?;
            Ok(MessageSkeleton {
                provider_msg_id: r.provider_msg_id,
                previously_cached: r.previously_cached,
            })
        })
    }

    /// backfill a message by coalescing all attributes and
    pub async fn backfill_message(&self, msg: Message) -> Result<(), AppError> {
        let mut tx = self.db_pool.begin().await?;

        sqlx::query!(
            "UPDATE messages SET

            thread_id = COALESCE(?, thread_id),
            sync_cursor = COALESCE(?, sync_cursor),
            internal_date = COALESCE(?, internal_date),
            size_estimate = COALESCE(?, size_estimate),

            date_header = COALESCE(?, date_header),
            from_addr = COALESCE(?, from_addr),
            to_addrs = COALESCE(?, to_addrs),
            cc_addrs = COALESCE(?, cc_addrs),

            -- in_reply_to and msg_references left out

            subject = COALESCE(?, subject),
            snippet = COALESCE(?, snippet)

            WHERE account_id = ? AND provider_msg_id = ?",
            msg.thread_id,
            msg.sync_cursor,
            msg.internal_date,
            msg.size_estimate,
            msg.date_header,
            msg.from_addr,
            msg.to_addrs,
            msg.cc_addrs,
            msg.subject,
            msg.snippet,
            self.account_id,
            msg.provider_msg_id,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            error!("failed to insert skeleton msg: {e:?}");
            AppError::Sqlx(e.into())
        })?;

        // delete, then readd all labels
        sqlx::query!(
            "DELETE FROM message_has_label WHERE provider_msg_id = ? AND account_id = ?",
            msg.provider_msg_id,
            self.account_id
        )
        .execute(&mut *tx)
        .await?;

        for l in msg.label_ids {
            sqlx::query!(
                "INSERT INTO message_has_label
                (account_id, provider_msg_id, label_id)
                VALUES (?, ?, ?)
                ",
                self.account_id,
                msg.provider_msg_id,
                l
            )
            .execute(&mut *tx)
            .await?;
        }

        for content in msg.contents {
            Self::store_message_contents(content, self.account_id, &mut *tx).await?;
        }

        tx.commit().await?;
        Ok(())
    }

    async fn store_message_contents(
        contents: MessageContents,
        account_id: i64,
        tx: impl Executor<'_, Database = Sqlite>,
    ) -> Result<(), AppError> {
        sqlx::query!(
            "INSERT INTO message_contents
        (account_id, provider_msg_id, mime_type, body)
        VALUES (?, ?, ?, ?)
        ON CONFLICT (account_id, provider_msg_id, mime_type)
        DO UPDATE SET body = excluded.body",
            account_id,
            contents.provider_msg_id,
            contents.mime_type,
            contents.body
        )
        .execute(tx)
        .await
        .map_err(|e| {
            error!("failed to insert skeleton msg: {e:?}");
            AppError::Sqlx(e.into())
        })
        .map(|_| ())
    }

    pub async fn get_account_config(&self) -> Result<AccountConfig, AppError> {
        sqlx::query!(
            "SELECT account_config FROM email_accounts WHERE id = ? LIMIT 1",
            self.account_id
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| {
            error!("failed to insert skeleton msg: {e:?}");
            AppError::Sqlx(e.into())
        })
        .and_then(|it| {
            serde_json::from_str(&it.account_config).map_err(|e| {
                error!("failed to parse account config: {e:?}");
                AppError::SerdeJson
            })
        })
    }

    pub async fn set_account_config(&self, config: &AccountConfig) -> Result<(), AppError> {
        let str = serde_json::to_string(config).map_err(|e| {
            error!("failed to parse account config: {e:?}");
            AppError::SerdeJson
        })?;

        sqlx::query!(
            "UPDATE email_accounts SET account_config = ? WHERE id = ?",
            str,
            self.account_id
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| {
            error!("failed to insert skeleton msg: {e:?}");
            AppError::Sqlx(e.into())
        })
        .map(|_| ())
    }

    // update the sync cursor for the account config to `to`
    pub async fn set_account_config_sync_cursor(&self, to: String) -> Result<(), AppError> {
        let mut tx = self.db_pool.begin().await?;

        let acc = sqlx::query!(
            "SELECT account_config FROM email_accounts WHERE id = ?",
            self.account_id
        )
        .fetch_one(&mut *tx)
        .await?;

        let config = serde_json::from_str::<AccountConfig>(&acc.account_config).map_err(|e| {
            error!("failed to parse account config: {e:?}");
            AppError::SerdeJson
        })?;

        let new_config = match config {
            AccountConfig::Gmail { history_id, scopes } => AccountConfig::Gmail {
                history_id: HistoryID::LastSynced(to),
                scopes,
            },
        };

        let new_config_json = serde_json::to_string(&new_config).map_err(|e| {
            error!("failed to serialize account config: {e:?}");
            AppError::SerdeJson
        })?;

        sqlx::query!(
            "UPDATE email_accounts SET account_config = ? WHERE id = ?",
            new_config_json,
            self.account_id
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        Ok(())
    }

    pub async fn get_latest_sync_cursor(&self) -> Result<String, AppError> {
        sqlx::query!(
            "SELECT sync_cursor FROM messages WHERE account_id = ? AND sync_cursor IS NOT NULL ORDER BY internal_date DESC LIMIT 1",
            self.account_id
        )
        .fetch_one(&self.db_pool)
        .await
        .map_err(|e| {
            error!("in full_sync: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })
        .and_then(|it| {
            it.sync_cursor.ok_or(AppError::GmailErr(GmailError::MissingField(MissingField::SyncCursor)))
        })
    }
}
