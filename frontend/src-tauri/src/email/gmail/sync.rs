use futures::StreamExt;
use log::{error, info};
use tauri::ipc::Channel;

use crate::email::gmail::repo::MessageStream;
use crate::email::repo::{
    AccountConfig, HistoryID, Label, MessageSkeleton, MissingField, NeedsCacheInvalidate,
};
use crate::{AppError, InvalidateEvent, Progress, ProgressReporter};

use super::{GmailApiError, GmailClient, GmailError};

mod full;
mod partial;

impl GmailClient {
    /// Synchronize this GmailClient with the Gmail API,
    /// follows [https://developers.google.com/workspace/gmail/api/guides/sync]
    ///
    /// Either performs a partial sync, or if not possible, a full sync.
    ///
    /// Returns the frontend caches that need to be invalidated.
    pub async fn sync(
        &self,
        update_channel: Channel<Progress>,
    ) -> Result<Vec<InvalidateEvent>, AppError> {
        update_channel.report(Progress::Update {
            completed: 0,
            out_of: Some(3),
        });

        update_channel.report_message("Loading Labels".into());
        let mut events = Vec::new();
        if let NeedsCacheInvalidate::Yes = self.sync_labels().await? {
            InvalidateEvent::Accounts.add_to(&mut events);
        }

        update_channel.report(Progress::Update {
            completed: 1,
            out_of: Some(3),
        });

        info!("synced labels");

        let AccountConfig::Gmail { history_id, .. } = self.repo.get_account_config().await?;
        info!("got account config! history_id: {:?}", history_id);

        let res = match history_id {
            HistoryID::LastSynced(to) => {
                info!("performing partial sync");

                let start_history_id = to.parse().map_err(|_| AppError::ParseAccountID)?;

                let res = self
                    .partial_sync(start_history_id, update_channel.clone())
                    .await;

                match res {
                    Ok(_) => {
                        info!("partial sync succeeded");
                        res
                    }

                    Err(e) => {
                        // if the partial sync fails, fall back to full sync
                        error!("partial sync failed: {e:?}, trying full sync");
                        self.full_sync(update_channel.clone()).await
                    }
                }
            }
            HistoryID::KnownStale => {
                info!("history_id is known stale, performing full sync");
                self.full_sync(update_channel.clone()).await
            }
        };

        update_channel.report_message(
            if res.is_ok() {
                "Finished Sync"
            } else {
                "Failed Sync"
            }
            .into(),
        );

        res.map(|sync_events| {
            for e in sync_events {
                e.add_to(&mut events);
            }
            events
        })
    }

    /// collect the invalidations for all of this account's messages if `status` is Yes
    fn add_message_events(&self, status: NeedsCacheInvalidate, events: &mut Vec<InvalidateEvent>) {
        if let NeedsCacheInvalidate::Yes = status {
            let account_id = self.account_id.to_string();
            InvalidateEvent::ListMessages {
                account_id: account_id.clone(),
            }
            .add_to(events);
            InvalidateEvent::GetMessage { account_id }.add_to(events);
        }
    }

    /// labels are used for sorting messages into inboxes as well as user defined labels
    /// this function refetches the user's available labels and reports whether the frontend labels query must be invalidated
    async fn sync_labels(&self) -> Result<NeedsCacheInvalidate, AppError> {
        // list the labels from the gmail api
        let (_, res) = self
            .client
            .users()
            .labels_list("me")
            .doit()
            .await
            .map_err(|e| {
                error!("failed to list labels: {e:?}");
                GmailApiError::from(e)
            })?;

        let Some(labels) = res.labels else {
            error!("gmail sync no labels returned");
            return Err(AppError::GmailMissingLabels);
        };

        let mut label_status = NeedsCacheInvalidate::No;
        for label in labels {
            // extract the required fields
            let google_gmail1::api::Label {
                id: Some(id),
                name: Some(name),
                message_list_visibility,
                label_list_visibility,
                type_: Some(type_),
                ..
            } = label
            else {
                error!("label missing required fields: {:?}", label);
                return Err(AppError::from(GmailError::MissingField(
                    MissingField::InLabel,
                )));
            };

            let status = self
                .repo
                .add_label(Label {
                    id,
                    name,
                    message_list_visibility,
                    label_list_visibility,
                    type_,
                })
                .await?;

            if matches!(status, NeedsCacheInvalidate::Yes) {
                label_status = NeedsCacheInvalidate::Yes;
            }
        }

        if matches!(label_status, NeedsCacheInvalidate::Yes) {
            info!("New Label was added");
        }

        Ok(label_status)
    }

    async fn backfill_messages(
        &self,
        msgs: MessageStream<impl futures::Stream<Item = Result<MessageSkeleton, AppError>>>,
        updates: Channel<Progress>,
    ) -> Result<NeedsCacheInvalidate, AppError> {
        let MessageStream { stream, total } = msgs;

        info!(
            "starting backfill for account_id={} n_messages={total}",
            self.account_id
        );

        let completed = std::sync::atomic::AtomicU64::new(0);
        let mut any_stored = false;

        // fetch the messages in parallel and load them into the db
        stream
            .map(|it| async {
                let s = match it {
                    Ok(s) => s,
                    Err(e) => {
                        error!("failed to read message skeleton: {e:?}");
                        return false;
                    }
                };

                // gather message id for better logging
                let id = s.provider_msg_id.clone();

                match self.fetch_and_store_skeleton(s).await {
                    Ok(()) => true,
                    Err(e) => {
                        error!("failed to load message id={id}: {e:?}");
                        false
                    }
                }
            })
            .buffer_unordered(GmailClient::N_FETCH_WORKERS)
            .for_each(|stored| {
                any_stored |= stored;
                let n = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                updates.report(Progress::Update {
                    completed: n,
                    out_of: Some(total.max(n)),
                });

                async move {}
            })
            .await;

        Ok(if any_stored {
            NeedsCacheInvalidate::Yes
        } else {
            NeedsCacheInvalidate::No
        })
    }
}
