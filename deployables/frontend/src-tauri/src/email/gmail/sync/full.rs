use std::collections::HashSet;

use log::{error, info, trace};
use tauri::ipc::Channel;

use crate::email::repo::{AccountConfig, HistoryID, MessageSkeleton};
use crate::{AppError, InvalidateEvent, Progress, ProgressReporter};

use super::{GmailApiError, GmailClient};

impl GmailClient {
    // https://developers.google.com/workspace/gmail/api/guides/sync#full-sync
    // todo: return u64
    pub async fn full_sync(
        &self,
        updates: Channel<Progress>,
    ) -> Result<Vec<InvalidateEvent>, AppError> {
        // TODO: include spam/trash? decide: lazy sync inboxes?

        // capture the mailbox historyId BEFORE listing, so changes made mid-sync aren't lost
        let (_, profile) = self
            .client
            .users()
            .get_profile("me")
            .doit()
            .await
            .map_err(GmailApiError::from)?;
        let Some(history_id) = profile.history_id else {
            error!("get_profile returned no history_id {profile:#?}");
            return Err(AppError::GmailResponseIncomplete);
        };

        updates.report_message("Full Sync: Fetching Emails".into());
        let listed_ids = self
            .fetch_and_store_message_skeletons(updates.clone())
            .await?;

        let mut events = Vec::new();
        // drop messages deleted remotely, before backfill so they aren't refetched
        let status = self.repo.delete_messages_not_in(&listed_ids).await?;
        self.add_message_events(status, &mut events);

        // refetch all messages
        updates.report_message("Loading Emails".into());
        let status = self
            .backfill_messages(self.repo.stream_all_messages().await?, updates.clone())
            .await?;
        self.add_message_events(status, &mut events);

        info!(
            "full_sync: finished backfilling all messages for account_id={}",
            self.account_id
        );
        let config = self.repo.get_account_config().await?;

        let AccountConfig::Gmail { scopes, .. } = config;

        let new_config = AccountConfig::Gmail {
            history_id: HistoryID::LastSynced(history_id.to_string()),
            scopes,
        };

        info!(
            "full sync: updated account config account_id={}, new config={new_config:?}",
            self.account_id
        );

        self.repo.set_account_config(&new_config).await?;

        Ok(events)
    }

    /// Synchronize and store the message skeletons from users.messages.list,
    /// returning the ids of every listed message
    async fn fetch_and_store_message_skeletons(
        &self,
        progress: Channel<Progress>,
    ) -> Result<HashSet<String>, AppError> {
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
        let mut listed_ids: HashSet<String> =
            messages.iter().filter_map(|m| m.id.clone()).collect();

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
                error!("messages_list returned no messages field {b:#?} {res:#?}",);
                if res.next_page_token.is_none() {
                    break;
                } else {
                    return Err(AppError::GmailResponseIncomplete);
                }
            };

            // and insert them into the table to be stored later
            self.repo.insert_skeleton_messages(&messages).await?;
            listed_ids.extend(messages.iter().filter_map(|m| m.id.clone()));

            next_page_token = res.next_page_token;
        }

        trace!(
            "full_sync: finished listing all messages for account_id={}",
            self.account_id
        );

        Ok(listed_ids)
    }

    /// fetch, parse and store a single message, backfilling a message skeleton
    /// this may also update the labelIds of a previously cached message
    pub(super) async fn fetch_and_store_skeleton(
        &self,
        s: MessageSkeleton,
    ) -> Result<(), AppError> {
        let req = self
            .client
            .users()
            .messages_get("me", &s.provider_msg_id)
            .format(if s.previously_cached {
                "minimal"
            } else {
                "full"
            });

        let (_, msg) = req.doit().await?;

        let message = GmailClient::parse_message(msg)?;

        self.repo.backfill_message(message).await
    }
}
