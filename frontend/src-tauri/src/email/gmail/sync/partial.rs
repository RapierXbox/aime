use google_gmail1::api::{History, ListHistoryResponse};
use google_gmail1::hyper;
use log::{error, info, warn};
use tauri::ipc::Channel;

use crate::email::repo::HistoryID;
use crate::{email, AppError, Progress, ProgressReporter};

use super::GmailClient;

impl GmailClient {
    pub(super) async fn partial_sync(
        &self,
        start_history_id: u64,
        updates: Channel<Progress>,
    ) -> Result<(), AppError> {
        // first invalidate the sync cursor to avoid failed syncs messing up data
        self.repo
            .set_account_config_sync_cursor(HistoryID::KnownStale)
            .await?;

        let res = self
            .client
            .users()
            .history_list("me")
            .start_history_id(start_history_id)
            .doit()
            .await;

        updates.report(Progress::Update {
            completed: 2,
            out_of: Some(4),
        });

        match res {
            Err(e) => match e {
                // on 404, we need to perform a full_sync
                google_gmail1::Error::Failure(res)
                    if res.status() == hyper::StatusCode::NOT_FOUND =>
                {
                    warn!("got a 404 {res:?} on users.history.list. Performing full sync");
                    return Err(AppError::GmailResponseIncomplete);
                }
                _ => return Err(e.into()),
            },

            // no updates since the last sync
            Ok((
                _,
                ListHistoryResponse {
                    history: None,
                    history_id: Some(history_id_new),
                    ..
                },
            )) => {
                updates.report(Progress::Update {
                    completed: 3,
                    out_of: Some(3),
                });

                info!("got empty history list, completing!");
                self.repo
                    .set_account_config_sync_cursor(HistoryID::LastSynced(
                        history_id_new.to_string(),
                    ))
                    .await?;
                return Ok(());
            }

            // got some history, apply it
            Ok((
                _,
                ListHistoryResponse {
                    history: Some(history),
                    history_id: Some(history_id_new),
                    next_page_token,
                    ..
                },
            )) => {
                self.apply_partial_sync_history(
                    start_history_id,
                    updates.clone(),
                    history,
                    history_id_new,
                    next_page_token,
                )
                .await?;
            }

            Ok((d, r)) => {
                error!("got messages.list response, didnt match correctly! r={r:#?}");
                return Err(AppError::GmailResponseIncomplete);
            }
        }

        // the added messages need to be loaded
        updates.report_message("Loading Emails".into());
        self.backfill_messages(self.repo.stream_message_skeletons().await?, updates)
            .await?;

        Ok(())
    }

    async fn apply_partial_sync_history(
        &self,
        start_history_id: u64,
        updates: Channel<Progress>,
        history: Vec<History>,
        history_id_new: u64,
        next_page_token: Option<String>,
    ) -> Result<(), AppError> {
        // update the history we already have
        self.repo.apply_history(history, updates.clone()).await?;
        // next up, check if there are more pages to fetch

        let mut running_page_token = next_page_token;
        while let Some(ref token) = running_page_token {
            info!(
                "incr sync res_history_id={history_id_new}, target={start_history_id} page 1 done"
            );

            let (_, res) = self
                .client
                .users()
                .history_list("me")
                .page_token(token)
                // use the history_id from the request response
                .start_history_id(start_history_id)
                .doit()
                .await?;

            match res {
                ListHistoryResponse {
                    history: Some(paged_history),
                    next_page_token,
                    ..
                } => {
                    self.repo
                        .apply_history(paged_history, updates.clone())
                        .await?;
                    running_page_token = next_page_token;
                }

                ListHistoryResponse {
                    history: None,
                    history_id: Some(history_id_finished),
                    ..
                } => {
                    updates.report_message("Loading Emails".into());
                    self.backfill_messages(
                        self.repo.stream_message_skeletons().await?,
                        updates.clone(),
                    )
                    .await?;

                    self.repo
                        .set_account_config_sync_cursor(email::repo::HistoryID::LastSynced(
                            history_id_finished.to_string(),
                        ))
                        .await?;

                    return Ok(());
                }

                _ => return Err(AppError::GmailResponseIncomplete),
            }
        }

        info!("finished partial sync to {history_id_new}");
        self.repo
            .set_account_config_sync_cursor(email::repo::HistoryID::LastSynced(
                history_id_new.to_string(),
            ))
            .await?;

        Ok(())
    }
}
