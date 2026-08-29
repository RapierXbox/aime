use futures::StreamExt;
use log::{error, info};
use tauri::ipc::Channel;

use crate::email::gmail::repo::MessageStream;
use crate::email::repo::{
    AccountConfig, AddLabelStatus, HistoryID, Label, MessageSkeleton, MissingField,
};
use crate::{AppError, Progress, ProgressReporter};

use super::{GmailApiError, GmailClient, GmailError};

mod full;
mod partial;

impl GmailClient {
    /// Synchronize this GmailClient with the Gmail API,
    /// follows [https://developers.google.com/workspace/gmail/api/guides/sync]
    ///
    /// Either performs a partial sync, or if not possible, a full sync.
    pub async fn sync(&self, update_channel: Channel<Progress>) -> Result<(), AppError> {
        update_channel.report(Progress::Update {
            completed: 0,
            out_of: Some(3),
        });

        update_channel.report_message("Loading Labels".into());
        self.sync_labels().await?;

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

        res
    }

    /// labels are used for sorting messages into inboxes as well as user defined labels
    /// this function refetches the user's available labels and (will) invalidate the frontend labels query
    async fn sync_labels(&self) -> Result<(), AppError> {
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

        let mut label_status = AddLabelStatus::AlreadyExists;
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

            if matches!(status, AddLabelStatus::Inserted) {
                label_status = AddLabelStatus::Inserted;
            }
        }

        if matches!(label_status, AddLabelStatus::Inserted) {
            info!("New Label was added");
            // TODO: invalidate frontend labels
        }

        Ok(())
    }

    async fn backfill_messages(
        &self,
        msgs: MessageStream<impl futures::Stream<Item = Result<MessageSkeleton, AppError>>>,
        updates: Channel<Progress>,
    ) -> Result<(), AppError> {
        let MessageStream { stream, total } = msgs;

        info!(
            "starting backfill for account_id={} n_messages={total}",
            self.account_id
        );

        let completed = std::sync::atomic::AtomicU64::new(0);

        // fetch the messages in parallel and load them into the db
        stream
            .map(|it| async {
                let s = match it {
                    Ok(s) => s,
                    Err(e) => return error!("failed to read message skeleton: {e:?}"),
                };

                // gather message id for better logging
                let id = s.provider_msg_id.clone();

                if let Err(e) = self.fetch_and_store_skeleton(s).await {
                    error!("failed to load message id={id}: {e:?}");
                }
            })
            .buffer_unordered(GmailClient::N_FETCH_WORKERS)
            .for_each(|()| {
                let n = completed.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                updates.report(Progress::Update {
                    completed: n,
                    out_of: Some(total.max(n)),
                });

                async move {}
            })
            .await;

        Ok(())
    }
}
