use std::{marker::PhantomData, process::Output};

use futures::StreamExt;
use google_gmail1::api::{
    History, HistoryLabelAdded, HistoryLabelRemoved, HistoryMessageAdded, HistoryMessageDeleted,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{Connection, Executor, Sqlite, Transaction};
use tauri::ipc::Channel;

use crate::{
    email::{
        gmail::GmailError,
        repo::{
            AccountConfig, AddLabelStatus, HistoryID, Label, Message, MessageContents,
            MessageSkeleton, MissingField,
        },
        MailBox,
    },
    AppError, DbPool, Progress,
};

/// A message stream together with the row count it was opened with.
///
/// `total` is a *hint*: it is counted just before the cursor opens, and backfill
/// workers keep writing to `messages` while the stream drains. Clamp it at the
/// consumer rather than trusting it to bound `completed`.
pub struct MessageStream<S> {
    pub stream: S,
    pub total: u64,
}

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

    /// All messages of this account, hydrated or not.
    pub async fn stream_all_messages(
        &self,
    ) -> Result<
        MessageStream<impl futures::Stream<Item = Result<MessageSkeleton, AppError>> + use<'_>>,
        AppError,
    > {
        let total = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!: i64" FROM messages WHERE account_id = ?"#,
            self.account_id
        )
        .fetch_one(&self.db_pool)
        .await
        .map(|c| c as u64)
        .map_err(|e| {
            error!("failed to count messages: {e:?}");
            AppError::Sqlx(e.into())
        })?;

        let stream = sqlx::query!(
            r#"SELECT provider_msg_id, internal_date IS NOT NULL as "previously_cached!: bool" FROM messages WHERE account_id = ?"#,
            self.account_id
        )
        .fetch(&self.db_pool)
        .map(|it| {
            let r = it?;
            Ok(MessageSkeleton {
                provider_msg_id: r.provider_msg_id,
                previously_cached: r.previously_cached,
            })
        });

        Ok(MessageStream { stream, total })
    }

    /// Messages that were listed but never hydrated (`internal_date IS NULL`).
    pub async fn stream_message_skeletons(
        &self,
    ) -> Result<
        MessageStream<impl futures::Stream<Item = Result<MessageSkeleton, AppError>> + use<'_>>,
        AppError,
    > {
        let total = sqlx::query_scalar!(
            r#"SELECT COUNT(*) as "count!: i64" FROM messages WHERE internal_date IS NULL AND account_id = ?"#,
            self.account_id
        )
        .fetch_one(&self.db_pool)
        .await
        .map(|c| c as u64)
        .map_err(|e| {
            error!("failed to count message skeletons: {e:?}");
            AppError::Sqlx(e.into())
        })?;

        let stream = sqlx::query!(
            r#"SELECT provider_msg_id
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
                previously_cached: false,
            })
        });

        Ok(MessageStream { stream, total })
    }

    /// backfill a message by coalescing all attributes and inserting it into the db
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
            error!("failed to backfill message row: {e:?}");
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
            error!("failed to insert message contents: {e:?}");
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
            error!("failed to read account config: {e:?}");
            AppError::Sqlx(e.into())
        })
        .and_then(|it| {
            serde_json::from_str(&it.account_config).map_err(|e| {
                error!("failed to parse account config: {e:?}");
                AppError::from(e)
            })
        })
    }

    pub async fn set_account_config(&self, config: &AccountConfig) -> Result<(), AppError> {
        let str = serde_json::to_string(config).map_err(|e| {
            error!("failed to serialize account config: {e:?}");
            AppError::from(e)
        })?;

        sqlx::query!(
            "UPDATE email_accounts SET account_config = ? WHERE id = ?",
            str,
            self.account_id
        )
        .execute(&self.db_pool)
        .await
        .map_err(|e| {
            error!("failed to write account config: {e:?}");
            AppError::Sqlx(e.into())
        })
        .map(|_| ())
    }

    // update the sync cursor for the account config to `to`
    pub async fn set_account_config_sync_cursor(&self, to: HistoryID) -> Result<(), AppError> {
        let mut tx = self.db_pool.begin().await?;

        let acc = sqlx::query!(
            "SELECT account_config FROM email_accounts WHERE id = ?",
            self.account_id
        )
        .fetch_one(&mut *tx)
        .await?;

        let config = serde_json::from_str::<AccountConfig>(&acc.account_config).map_err(|e| {
            error!("failed to parse account config: {e:?}");
            AppError::from(e)
        })?;

        let new_config = match config {
            AccountConfig::Gmail { history_id, scopes } => AccountConfig::Gmail {
                history_id: to,
                scopes,
            },
        };

        let new_config_json = serde_json::to_string(&new_config).map_err(|e| {
            error!("failed to serialize account config: {e:?}");
            AppError::from(e)
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
            error!("in get_latest_sync_cursor: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })
        .and_then(|it| {
            it.sync_cursor.ok_or(AppError::GmailErr(GmailError::MissingField(MissingField::SyncCursor)))
        })
    }

    const PAGE_SIZE: u32 = 100;

    // TODO: return labels? (extra query per message), read status
    pub async fn list_messages(
        &self,
        mailbox: MailBox,
        page_index: u32,
    ) -> Result<Vec<Message>, AppError> {
        sqlx::query!(
            r#"SELECT ROW_NUMBER() OVER (ORDER BY internal_date DESC) AS "row_num!: i64",
                m.provider_msg_id,
                m.size_estimate as "size_estimate: i32",
                m.thread_id,
                m.sync_cursor,
                m.internal_date,
                m.date_header,
                m.from_addr,
                m.to_addrs,
                m.cc_addrs,
                m.subject,
                m.snippet
            FROM messages AS m
            JOIN message_has_label AS ml ON ml.provider_msg_id = m.provider_msg_id
            JOIN labels AS l ON l.id = ml.label_id
                        WHERE m.account_id = ?
                AND l.name = ?
                LIMIT ? OFFSET ?"#,
            self.account_id,
            mailbox,
            Self::PAGE_SIZE,
            page_index
        )
        .fetch_all(&self.db_pool)
        .await
        .map_err(|e| {
            error!("in list_messages: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })
        .map(|it| {
            it.into_iter()
                .map(|re| Message {
                    provider_msg_id: Some(re.provider_msg_id),
                    label_ids: vec![], // TODO
                    contents: vec![],
                    size_estimate: re.size_estimate,
                    thread_id: re.thread_id,
                    sync_cursor: re.sync_cursor,
                    internal_date: re.internal_date,
                    date_header: re.date_header,
                    from_addr: re.from_addr,
                    to_addrs: re.to_addrs,
                    cc_addrs: re.cc_addrs,
                    subject: re.subject,
                    snippet: re.snippet,
                })
                .collect()
        })
    }
}
