pub mod model;
pub mod service;
pub mod store;
#[cfg(test)]
mod tests;

use crate::{
    email::{gmail::GmailClient, EmailManager},
    AppError, DbPool, InvalidateEvent,
};
use model::*;
use service::Service;
use std::sync::Arc;
use store::Store;
use tauri::{AppHandle, State};
use tauri_specta::Event;

async fn account(m: &EmailManager, id: &str) -> Result<Arc<GmailClient>, AppError> {
    m.get_client(id.parse().map_err(|_| AppError::ParseAccountID)?)
        .await
        .ok_or(AppError::AccountNotFound)
}
async fn owner(m: &EmailManager, db: &DbPool, id: &str) -> Result<Arc<GmailClient>, AppError> {
    let d = Store(db.clone()).get(id).await?;
    account(m, &d.account_id).await
}
fn service<'a>(client: &'a GmailClient, db: &DbPool) -> Service<'a> {
    Service {
        store: Store(db.clone()),
        provider: client,
        sender: client.email_addr(),
        account: client.account_id(),
    }
}
fn invalidate(app: &AppHandle, account: &str) {
    let _ = (InvalidateEvent::DraftsChanged {
        account_id: account.into(),
    })
    .emit(app);
}
#[tauri::command]
#[specta::specta]
pub async fn create_draft(
    account_id: String,
    mode: ComposeMode,
    source_message_id: Option<String>,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<Draft, AppError> {
    let c = account(&email, &account_id).await?;
    let d = service(&c, &db).create(mode, source_message_id).await?;
    invalidate(&app, &account_id);
    Ok(d)
}
#[tauri::command]
#[specta::specta]
pub async fn list_drafts(
    account_id: String,
    db: State<'_, DbPool>,
) -> Result<Vec<Draft>, AppError> {
    Ok(Store(db.inner().clone())
        .list(account_id.parse().map_err(|_| AppError::ParseAccountID)?)
        .await?
        .into_iter()
        .filter(|d| !matches!(d.status, DraftStatus::Sent | DraftStatus::Deleted))
        .collect())
}
#[tauri::command]
#[specta::specta]
pub async fn get_draft(draft_id: String, db: State<'_, DbPool>) -> Result<Draft, AppError> {
    Store(db.inner().clone()).get(&draft_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn save_draft(
    draft_id: String,
    expected_revision: u32,
    fields: DraftFields,
    db: State<'_, DbPool>,
    app: AppHandle,
) -> Result<Draft, AppError> {
    let d = Store(db.inner().clone())
        .save(&draft_id, expected_revision, fields)
        .await?;
    invalidate(&app, &d.account_id);
    Ok(d)
}
#[tauri::command]
#[specta::specta]
pub async fn sync_drafts(
    account_id: String,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<(), AppError> {
    let c = account(&email, &account_id).await?;
    let _guard = c.draft_lock.lock().await;
    let result = service(&c, &db).sync().await;
    invalidate(&app, &account_id);
    result.inspect_err(|e| log::error!("draft sync for account {account_id} failed: {e}"))
}
#[tauri::command]
#[specta::specta]
pub async fn resolve_draft_conflict(
    draft_id: String,
    expected_revision: u32,
    choice: ConflictChoice,
    local_fields: Option<DraftFields>,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<Draft, AppError> {
    let c = owner(&email, &db, &draft_id).await?;
    let _guard = c.draft_lock.lock().await;
    let d = service(&c, &db)
        .conflict(&draft_id, expected_revision, choice, local_fields)
        .await?;
    invalidate(&app, &d.account_id);
    Ok(d)
}
#[tauri::command]
#[specta::specta]
pub async fn delete_draft(
    draft_id: String,
    expected_revision: u32,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<(), AppError> {
    let c = owner(&email, &db, &draft_id).await?;
    let _guard = c.draft_lock.lock().await;
    let result = service(&c, &db).delete(&draft_id, expected_revision).await;
    invalidate(&app, &c.account_id().to_string());
    result
}
#[tauri::command]
#[specta::specta]
pub async fn send_draft(
    draft_id: String,
    expected_revision: u32,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<Draft, AppError> {
    let c = owner(&email, &db, &draft_id).await?;
    let _guard = c.draft_lock.lock().await;
    let result = service(&c, &db).send(&draft_id, expected_revision).await;
    invalidate(&app, &c.account_id().to_string());
    if let Ok(d) = &result {
        if let Some(id) = d.sent_message_id.clone() {
            let client = c.clone();
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if client.cache_sent(&id).await.is_ok() {
                    let _ = (InvalidateEvent::ListMessages {
                        account_id: client.account_id().to_string(),
                    })
                    .emit(&app);
                    let _ = (InvalidateEvent::GetMessage {
                        account_id: client.account_id().to_string(),
                    })
                    .emit(&app);
                }
            });
        }
    }
    result
}
#[tauri::command]
#[specta::specta]
pub async fn resolve_send_attempt(
    attempt_id: String,
    resolution: SendResolution,
    db: State<'_, DbPool>,
    email: State<'_, EmailManager>,
    app: AppHandle,
) -> Result<Draft, AppError> {
    let a = Store(db.inner().clone()).attempt(&attempt_id).await?;
    let c = account(&email, &a.draft.account_id).await?;
    let _guard = c.draft_lock.lock().await;
    let d = service(&c, &db)
        .resolve_send(&attempt_id, resolution)
        .await?;
    invalidate(&app, &d.account_id);
    Ok(d)
}
#[tauri::command]
#[specta::specta]
pub async fn open_gmail(
    account_id: String,
    sent: bool,
    email: State<'_, EmailManager>,
) -> Result<(), AppError> {
    let c = account(&email, &account_id).await?;
    let mut url = oauth2::url::Url::parse("https://mail.google.com/mail/u/")
        .map_err(|_| AppError::UrlParseError)?;
    url.query_pairs_mut()
        .append_pair("authuser", c.email_addr());
    url.set_fragment(Some(if sent { "sent" } else { "drafts" }));
    open::that(url.as_str())?;
    Ok(())
}
