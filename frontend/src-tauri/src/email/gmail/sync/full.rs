use log::{error, info, trace};
use tauri::ipc::Channel;

use crate::email::repo::{AccountConfig, HistoryID, MessageSkeleton};
use crate::{AppError, Progress, ProgressReporter};

use super::{GmailApiError, GmailClient};

impl GmailClient {
    // https://developers.google.com/workspace/gmail/api/guides/sync#full-sync
    // todo: return u64
    // TODO: this misses deleted messages since the fetch_and_store_skeletons doesnt delete records that werent touched
    pub async fn full_sync(&self, updates: Channel<Progress>) -> Result<(), AppError> {
        // TODO: include spam/trash? decide: lazy sync inboxes?

        updates.report_message("Fetching Emails".into());
        self.fetch_and_store_message_skeletons(updates.clone())
            .await?;

        // refetch all messages
        updates.report_message("Loading Emails".into());
        self.backfill_messages(self.repo.stream_all_messages().await?, updates.clone())
            .await?;

        info!(
            "full_sync: finished backfilling all messages for account_id={}",
            self.account_id
        );
        // todo: should be the the first message in the messages.list response
        let latest_history_id = self.repo.get_latest_sync_cursor().await?;
        let config = self.repo.get_account_config().await?;

        let AccountConfig::Gmail { scopes, .. } = config;

        let new_config = AccountConfig::Gmail {
            history_id: HistoryID::LastSynced(latest_history_id),
            scopes,
        };

        info!(
            "full sync: updated account config account_id={}, new config={new_config:?}",
            self.account_id
        );

        self.repo.set_account_config(&new_config).await?;

        Ok(())
    }

    /// Synchronize and store the message skeletons from users.messages.list
    async fn fetch_and_store_message_skeletons(
        &self,
        progress: Channel<Progress>,
    ) -> Result<(), AppError> {
        // https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.messages/list
        // list the first 500 messages
        let (b, res) = self
            .client
            .users()
            .messages_list("me")
            .include_spam_trash(true)
            .max_results(500)
            .doit()
            .await
            .map_err(|e| {
                error!("failed to list messages: {e:#?}");
                GmailApiError::from(e)
            })?;

        let Some(messages) = &res.messages else {
            error!("messages_list returned no messages field {b:#?}");
            return Err(AppError::GmailResponseIncomplete);
        };

        self.repo.insert_skeleton_messages(&messages).await?;

        let mut next_page_token = res.next_page_token;
        // until no new page token is returned,
        let mut id = 1;
        while let Some(token) = next_page_token {
            id += 1;
            progress.report(Progress::Update {
                completed: id,
                out_of: Some(10.max(id + 2)),
            });

            info!("fetching messages page token={token}");

            // fetch the next 500 results
            let (b, res) = self
                .client
                .users()
                .messages_list("me")
                .include_spam_trash(true)
                .max_results(500)
                .page_token(&token)
                .doit()
                .await
                .map_err(|e| {
                    error!("failed to list messages: {e:#?}");
                    GmailApiError::from(e)
                })?;

            let Some(messages) = &res.messages else {
                error!("messages_list returned no messages field {b:#?}");
                return Err(AppError::GmailResponseIncomplete);
            };

            // and insert them into the table to be stored later
            self.repo.insert_skeleton_messages(&messages).await?;

            next_page_token = res.next_page_token;
        }

        trace!(
            "full_sync: finished listing all messages for account_id={}",
            self.account_id
        );

        Ok(())
    }

    /// fetch, parse and store a single message, backfilling a message skeleton
    /// this may also update the labelIds of a previously cached message
    pub(super) async fn fetch_and_store_skeleton(
        &self,
        s: MessageSkeleton,
    ) -> Result<(), AppError> {
        let (_, msg) = self
            .client
            .users()
            .messages_get("me", &s.provider_msg_id)
            .format(if s.previously_cached {
                "minimal"
            } else {
                "full"
            })
            .doit()
            .await?;

        let message = GmailClient::parse_message(msg)?;

        self.repo.backfill_message(message).await
    }
}
