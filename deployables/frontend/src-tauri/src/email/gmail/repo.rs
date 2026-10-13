use std::{
    collections::{HashMap, HashSet},
    marker::PhantomData,
    process::Output,
};

use futures::StreamExt;
use google_gmail1::api::{
    History, HistoryLabelAdded, HistoryLabelRemoved, HistoryMessageAdded, HistoryMessageDeleted,
};
use log::{error, info};
use serde::{Deserialize, Serialize};
use specta::Type;
use sqlx::{Connection, Executor, QueryBuilder, Sqlite, Transaction};
use tauri::{ipc::Channel, utils::mime_type::MimeType};

use crate::{
    email::{
        repo::{
            AccountConfig, HistoryID, Label, Message, MessageContents, MessageSkeleton,
            NeedsCacheInvalidate,
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

    pub async fn add_label(&self, label: Label) -> Result<NeedsCacheInvalidate, crate::AppError> {
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
            0 => NeedsCacheInvalidate::No,
            1 => NeedsCacheInvalidate::Yes,
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

        if msg.metadata_loaded {
            let metadata = serde_json::json!({
                "reply_to": msg.reply_to, "message_id": msg.message_id,
                "in_reply_to": msg.in_reply_to, "references": msg.references,
                "bcc_addrs": msg.bcc_addrs, "mime_issues": msg.mime_issues,
                "metadata_loaded": true
            }).to_string();
            sqlx::query("UPDATE messages SET compose_metadata = ?, in_reply_to = ?, msg_references = ? WHERE account_id = ? AND provider_msg_id = ?")
                .bind(metadata).bind(&msg.in_reply_to).bind(&msg.references)
                .bind(self.account_id).bind(&msg.provider_msg_id).execute(&mut *tx).await?;
        }

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

    /// Delete this account's messages whose id is not in `keep` (i.e. deleted remotely).
    /// `message_contents` and `message_has_label` rows follow via `ON DELETE CASCADE`.
    // ponytail: in-memory id set, ~50 bytes x mailbox size. Use a TEMP table if mailboxes reach millions.
    pub async fn delete_messages_not_in(
        &self,
        keep: &HashSet<String>,
    ) -> Result<NeedsCacheInvalidate, AppError> {
        let mut tx = self.db_pool.begin().await?;

        let local = sqlx::query!(
            "SELECT provider_msg_id FROM messages WHERE account_id = ?",
            self.account_id
        )
        .fetch_all(&mut *tx)
        .await?;

        let mut deleted = false;
        for row in local.iter().filter(|r| !keep.contains(&r.provider_msg_id)) {
            sqlx::query!(
                "DELETE FROM messages WHERE account_id = ? AND provider_msg_id = ?",
                self.account_id,
                row.provider_msg_id
            )
            .execute(&mut *tx)
            .await?;
            deleted = true;
        }

        tx.commit().await?;

        Ok(if deleted {
            NeedsCacheInvalidate::Yes
        } else {
            NeedsCacheInvalidate::No
        })
    }

    const PAGE_SIZE: u32 = 100;

    /// List messages in the mailbox, with pagination via page index.
    /// Returns at most [`Self::PAGE_SIZE`] messages, offset by `page_index` (=> real offset is `page_index * Self::PAGE_SIZE`).
    pub async fn list_messages(
        &self,
        mailbox: MailBox,
        page_index: u32,
    ) -> Result<ListMessages, AppError> {
        let page_offset = page_index * Self::PAGE_SIZE;
        let mut tx = self.db_pool.begin().await?;

        let rows = sqlx::query!(
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
            JOIN message_has_label AS ml ON ml.account_id = m.account_id
                AND ml.provider_msg_id = m.provider_msg_id
            JOIN labels AS l ON l.account_id = ml.account_id AND l.id = ml.label_id
            WHERE m.account_id = ?
                AND l.name = ?
                ORDER BY m.internal_date DESC
                LIMIT ? OFFSET ?"#,
            self.account_id,
            mailbox,
            Self::PAGE_SIZE,
            page_offset
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| {
            error!("in list_messages: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })?;

        let mut labels: HashMap<String, Vec<String>> = HashMap::new();
        if !rows.is_empty() {
            let mut query = QueryBuilder::<Sqlite>::new(
                "SELECT provider_msg_id, label_id FROM message_has_label WHERE account_id = ",
            );
            query
                .push_bind(self.account_id)
                .push(" AND provider_msg_id IN (");
            let mut ids = query.separated(", ");
            for row in &rows {
                ids.push_bind(&row.provider_msg_id);
            }
            ids.push_unseparated(")");

            for (msg_id, label_id) in query
                .build_query_as::<(String, String)>()
                .fetch_all(&mut *tx)
                .await?
            {
                labels.entry(msg_id).or_default().push(label_id);
            }
        }

        let result = ListMessages {
            next_page_param: if rows.len() == Self::PAGE_SIZE as usize {
                Some(page_index + 1)
            } else {
                None
            },
            messages: rows
                .into_iter()
                .map(|re| {
                    let label_ids = labels.remove(&re.provider_msg_id).unwrap_or_default();
                    Message {
                        provider_msg_id: Some(re.provider_msg_id),
                        label_ids,
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
                        ..Default::default()
                    }
                })
                .collect(),
        };

        tx.commit().await?;
        Ok(result)
    }

    pub(crate) async fn get_message(&self, msg_id: &str) -> Result<Message, AppError> {
        let mut tx = self.db_pool.begin().await?;
        let row = sqlx::query!(
            r#"SELECT provider_msg_id, size_estimate as "size_estimate: i32", thread_id,
                sync_cursor, internal_date, date_header, from_addr, to_addrs, cc_addrs,
                subject, snippet
            FROM messages
            WHERE account_id = ? AND provider_msg_id = ?"#,
            self.account_id,
            msg_id,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(|e| {
            error!("in get_message: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })?;

        let contents = sqlx::query_as!(
            MessageContents,
            "SELECT provider_msg_id, mime_type, body FROM message_contents
            WHERE account_id = ? AND provider_msg_id = ?",
            self.account_id,
            msg_id,
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(|e| {
            error!("in get_message: db returned error {e:?}");
            crate::AppError::Sqlx(e.into())
        })?;

        let label_ids = sqlx::query_scalar!(
            "SELECT label_id FROM message_has_label
             WHERE account_id = ? AND provider_msg_id = ?",
            self.account_id,
            msg_id,
        )
        .fetch_all(&mut *tx)
        .await?;

        tx.commit().await?;

        let metadata: Option<String> = sqlx::query_scalar("SELECT compose_metadata FROM messages WHERE account_id = ? AND provider_msg_id = ?")
            .bind(self.account_id).bind(msg_id).fetch_one(&self.db_pool).await?;
        let mut extra = serde_json::to_value(Message::default())?;
        if let Some(metadata) = metadata {
            if let (Some(target), Some(source)) = (extra.as_object_mut(), serde_json::from_str::<serde_json::Value>(&metadata)?.as_object()) { target.extend(source.clone()); }
        }
        let extra: Message = serde_json::from_value(extra)?;
        Ok(Message {
            provider_msg_id: Some(row.provider_msg_id),
            contents,
            label_ids,
            size_estimate: row.size_estimate,
            thread_id: row.thread_id,
            sync_cursor: row.sync_cursor,
            internal_date: row.internal_date,
            date_header: row.date_header,
            from_addr: row.from_addr,
            to_addrs: row.to_addrs,
            cc_addrs: row.cc_addrs,
            subject: row.subject,
            snippet: row.snippet,
            ..extra
        })
    }
}

#[derive(Debug, Type, Serialize, Deserialize)]
pub struct ListMessages {
    pub messages: Vec<Message>,
    pub next_page_param: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_label_then_readd_is_already_exists() {}

    #[test]
    fn backfill_message_coalesces_without_clobbering() {}

    /// In-memory db with the migrations applied and `n` INBOX messages inserted
    /// *oldest first*. Insertion order is therefore the exact reverse of what
    /// `list_messages` must return, so an unordered query fails the assertions
    /// instead of passing on natural rowid order.
    async fn repo_with_inbox_messages(n: u32) -> GmailRepo {
        let db_pool = DbPool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!().run(&db_pool).await.unwrap();

        sqlx::query!(
            "INSERT INTO email_accounts (id, email, account_name, account_config)
             VALUES (1, 'a@b.c', 'test', '{}')"
        )
        .execute(&db_pool)
        .await
        .unwrap();

        sqlx::query!(
            "INSERT INTO labels (account_id, id, name, type)
             VALUES (1, 'INBOX', 'INBOX', 'system')"
        )
        .execute(&db_pool)
        .await
        .unwrap();

        for i in 0..n {
            let msg_id = format!("msg-{i:04}");
            let internal_date = i as i64;
            sqlx::query!(
                "INSERT INTO messages (account_id, provider_msg_id, internal_date)
                 VALUES (1, ?, ?)",
                msg_id,
                internal_date
            )
            .execute(&db_pool)
            .await
            .unwrap();
            sqlx::query!(
                "INSERT INTO message_has_label (account_id, provider_msg_id, label_id)
                 VALUES (1, ?, 'INBOX')",
                msg_id
            )
            .execute(&db_pool)
            .await
            .unwrap();
        }

        GmailRepo::new(db_pool, 1)
    }

    #[tokio::test]
    async fn list_messages_sets_next_page_param_at_page_size() {
        let repo = repo_with_inbox_messages(GmailRepo::PAGE_SIZE).await;
        let page = repo.list_messages(MailBox::Inbox, 0).await.unwrap();

        assert_eq!(page.messages.len(), GmailRepo::PAGE_SIZE as usize);
        assert_eq!(page.next_page_param, Some(1));
    }

    #[tokio::test]
    async fn list_messages_no_next_page_below_page_size() {
        let repo = repo_with_inbox_messages(GmailRepo::PAGE_SIZE - 1).await;
        let page = repo.list_messages(MailBox::Inbox, 0).await.unwrap();

        assert_eq!(page.messages.len(), GmailRepo::PAGE_SIZE as usize - 1);
        assert_eq!(page.next_page_param, None);
    }

    #[tokio::test]
    async fn list_messages_attaches_labels_to_each_page() {
        let repo = repo_with_inbox_messages(GmailRepo::PAGE_SIZE + 1).await;
        for label in ["UNREAD", "STARRED"] {
            sqlx::query(
                "INSERT INTO labels (account_id, id, name, type) VALUES (1, ?, ?, 'system')",
            )
            .bind(label)
            .bind(label)
            .execute(&repo.db_pool)
            .await
            .unwrap();
        }
        for (msg_id, label) in [
            ("msg-0000", "UNREAD"),
            ("msg-0100", "UNREAD"),
            ("msg-0100", "STARRED"),
        ] {
            sqlx::query(
                "INSERT INTO message_has_label (account_id, provider_msg_id, label_id)
                 VALUES (1, ?, ?)",
            )
            .bind(msg_id)
            .bind(label)
            .execute(&repo.db_pool)
            .await
            .unwrap();
        }

        let first = repo.list_messages(MailBox::Inbox, 0).await.unwrap();
        let second = repo.list_messages(MailBox::Inbox, 1).await.unwrap();
        assert_eq!(first.messages.len(), GmailRepo::PAGE_SIZE as usize);
        assert_eq!(second.messages.len(), 1);
        for message in first.messages.iter().chain(&second.messages) {
            let msg_id = message.provider_msg_id.as_deref().unwrap();
            let mut labels = message.label_ids.clone();
            labels.sort();
            match msg_id {
                "msg-0100" => assert_eq!(labels, ["INBOX", "STARRED", "UNREAD"]),
                "msg-0000" => assert_eq!(labels, ["INBOX", "UNREAD"]),
                _ => assert_eq!(labels, ["INBOX"]),
            }
        }

        let empty = repo.list_messages(MailBox::Inbox, 2).await.unwrap();
        assert!(empty.messages.is_empty());
        assert_eq!(empty.next_page_param, None);
    }

    #[tokio::test]
    async fn message_labels_and_mailbox_membership_are_account_scoped() {
        let repo = repo_with_inbox_messages(2).await;
        sqlx::query(
            "DELETE FROM message_has_label WHERE account_id = 1 AND provider_msg_id = 'msg-0001'",
        )
        .execute(&repo.db_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO email_accounts (id, email, account_name, account_config)
             VALUES (2, 'other@b.c', 'other', '{}')",
        )
        .execute(&repo.db_pool)
        .await
        .unwrap();
        for label in ["INBOX", "UNREAD"] {
            sqlx::query(
                "INSERT INTO labels (account_id, id, name, type) VALUES (2, ?, ?, 'system')",
            )
            .bind(label)
            .bind(label)
            .execute(&repo.db_pool)
            .await
            .unwrap();
        }
        for msg_id in ["msg-0000", "msg-0001"] {
            sqlx::query(
                "INSERT INTO messages (account_id, provider_msg_id, internal_date) VALUES (2, ?, 42)",
            )
            .bind(msg_id)
            .execute(&repo.db_pool)
            .await
            .unwrap();
            for label in ["INBOX", "UNREAD"] {
                sqlx::query(
                    "INSERT INTO message_has_label (account_id, provider_msg_id, label_id)
                     VALUES (2, ?, ?)",
                )
                .bind(msg_id)
                .bind(label)
                .execute(&repo.db_pool)
                .await
                .unwrap();
            }
        }

        let page = repo.list_messages(MailBox::Inbox, 0).await.unwrap();
        assert_eq!(page.messages.len(), 1);
        assert_eq!(
            page.messages[0].provider_msg_id.as_deref(),
            Some("msg-0000")
        );
        assert_eq!(page.messages[0].label_ids, ["INBOX"]);
        assert_eq!(
            repo.get_message("msg-0000").await.unwrap().label_ids,
            ["INBOX"]
        );
        assert!(repo
            .get_message("msg-0001")
            .await
            .unwrap()
            .label_ids
            .is_empty());

        let other = GmailRepo::new(repo.db_pool.clone(), 2);
        let other_page = other.list_messages(MailBox::Inbox, 0).await.unwrap();
        assert_eq!(other_page.messages.len(), 2);
        assert!(other_page
            .messages
            .iter()
            .all(|m| m.label_ids.contains(&"UNREAD".into())));
    }

    #[tokio::test]
    async fn fetching_unread_message_preserves_stored_labels() {
        let repo = repo_with_inbox_messages(1).await;
        sqlx::query(
            "INSERT INTO labels (account_id, id, name, type) VALUES (1, 'UNREAD', 'UNREAD', 'system')",
        )
        .execute(&repo.db_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO message_has_label (account_id, provider_msg_id, label_id)
             VALUES (1, 'msg-0000', 'UNREAD')",
        )
        .execute(&repo.db_pool)
        .await
        .unwrap();

        let message = repo.get_message("msg-0000").await.unwrap();
        assert!(message.label_ids.contains(&"UNREAD".into()));
        let unread_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM message_has_label
             WHERE account_id = 1 AND provider_msg_id = 'msg-0000' AND label_id = 'UNREAD'",
        )
        .fetch_one(&repo.db_pool)
        .await
        .unwrap();
        assert_eq!(unread_count, 1);
        let page = repo.list_messages(MailBox::Inbox, 0).await.unwrap();
        assert!(page.messages[0].label_ids.contains(&"UNREAD".into()));

        // A later sync can remove UNREAD; the next list read must reflect it.
        sqlx::query(
            "DELETE FROM message_has_label
             WHERE account_id = 1 AND provider_msg_id = 'msg-0000' AND label_id = 'UNREAD'",
        )
        .execute(&repo.db_pool)
        .await
        .unwrap();
        let refreshed = repo.list_messages(MailBox::Inbox, 0).await.unwrap();
        assert_eq!(refreshed.messages[0].label_ids, ["INBOX"]);
    }

    /// Paging over an unordered query silently repeats or skips rows. Guards the
    /// explicit `ORDER BY m.internal_date DESC` that infinite scroll relies on.
    #[tokio::test]
    async fn list_messages_pages_do_not_overlap_and_stay_date_ordered() {
        let total = GmailRepo::PAGE_SIZE + GmailRepo::PAGE_SIZE / 2;
        let repo = repo_with_inbox_messages(total).await;

        let first = repo.list_messages(MailBox::Inbox, 0).await.unwrap();
        let second = repo.list_messages(MailBox::Inbox, 1).await.unwrap();

        assert_eq!(second.next_page_param, None);

        let all: Vec<_> = first
            .messages
            .iter()
            .chain(second.messages.iter())
            .collect();
        assert_eq!(all.len(), total as usize);

        let ids: std::collections::HashSet<_> =
            all.iter().map(|m| m.provider_msg_id.clone()).collect();
        assert_eq!(ids.len(), total as usize, "pages overlap");

        assert!(
            all.windows(2)
                .all(|w| w[0].internal_date >= w[1].internal_date),
            "not ordered by internal_date DESC across the page boundary"
        );
    }

    /// The decoy row under a different message is what fails if the
    /// `provider_msg_id` predicate is missing or wrong.
    #[tokio::test]
    async fn get_message_returns_only_its_own_contents() {
        let repo = repo_with_inbox_messages(2).await;

        for (msg_id, body) in [("msg-0000", "mine"), ("msg-0001", "decoy")] {
            sqlx::query!(
                "INSERT INTO message_contents (account_id, provider_msg_id, mime_type, body)
                 VALUES (1, ?, 'text/plain', ?)",
                msg_id,
                body
            )
            .execute(&repo.db_pool)
            .await
            .unwrap();
        }

        let msg = repo.get_message("msg-0000").await.unwrap();

        assert_eq!(msg.provider_msg_id.as_deref(), Some("msg-0000"));
        assert_eq!(msg.internal_date, Some(0));
        assert_eq!(msg.contents.len(), 1);
        assert_eq!(msg.contents[0].body, "mine");
    }

    #[tokio::test]
    async fn delete_messages_not_in_removes_only_missing() {
        let repo = repo_with_inbox_messages(3).await;
        for id in ["msg-0000", "msg-0001", "msg-0002"] {
            sqlx::query!(
                "INSERT INTO message_contents (account_id, provider_msg_id, mime_type, body)
                 VALUES (1, ?, 'text/plain', 'b')",
                id
            )
            .execute(&repo.db_pool)
            .await
            .unwrap();
        }

        let keep: HashSet<String> = ["msg-0000", "msg-0002"].map(String::from).into();
        let res = repo.delete_messages_not_in(&keep).await.unwrap();
        assert!(matches!(res, NeedsCacheInvalidate::Yes));

        // the deleted message's contents and label rows cascade away, the rest stay
        for (table, expected) in [
            ("messages", 2),
            ("message_contents", 2),
            ("message_has_label", 2),
        ] {
            let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(&repo.db_pool)
                .await
                .unwrap();
            assert_eq!(n, expected, "{table}");
        }
        let gone: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE provider_msg_id = 'msg-0001'")
                .fetch_one(&repo.db_pool)
                .await
                .unwrap();
        assert_eq!(gone, 0);

        // nothing left to delete
        let res = repo.delete_messages_not_in(&keep).await.unwrap();
        assert!(matches!(res, NeedsCacheInvalidate::No));
    }

    /// A skeleton row (listed, never hydrated) has no contents. The command must
    /// still succeed so the UI can tell "not downloaded yet" from an error.
    #[tokio::test]
    async fn get_message_on_skeleton_returns_empty_contents() {
        let repo = repo_with_inbox_messages(1).await;
        let msg = repo.get_message("msg-0000").await.unwrap();
        assert!(msg.contents.is_empty());
    }

    #[test]
    fn account_config_round_trips_through_json() {}
}
