use google_gmail1::{
    hyper_rustls::HttpsConnector,
    hyper_util::client::legacy::connect::{dns::GaiResolver, HttpConnector},
};
use tauri::State;

use crate::{
    email::{
        repo::{self, AccountConfig, EmailAccount},
        EmailManager,
    },
    DbPool,
};

pub type Gmail = google_gmail1::Gmail<HttpsConnector<HttpConnector<GaiResolver>>>;

pub mod auth;

#[tauri::command]
pub async fn register_gmail_account(
    app: tauri::AppHandle,
    db_pool: State<'_, DbPool>,
    email_mng: State<'_, EmailManager>,
) -> Result<(), crate::Error> {
    let client = email_mng.http_client.clone();

    let tmp_auth = auth::TempAuth::do_oauth2_flow(email_mng.oauth_reqwest_client.clone()).await?;

    let tmp_gmail = Gmail::new(client, tmp_auth.clone());
    let (_, profile) = tmp_gmail.users().get_profile("me").doit().await?;

    // TODO: use Arc<str> for emails since they are cloned frequently
    // TODO: error checks
    let email = profile.email_address.clone().unwrap();

    tmp_auth.save_to_keyring(&app, &email)?;

    let id = repo::add_account(
        &db_pool,
        EmailAccount {
            id: -1,
            email: email.clone(),
            account_name: profile.email_address.clone().unwrap(),
            account_config: AccountConfig::Gmail {
                history_id: profile.history_id.unwrap(),
                scopes: tmp_auth.scopes().to_vec(),
            },
        },
    )
    .await?;

    let auth = tmp_auth.promote(profile);
    // the gmail account with the authenticated user

    email_mng.account_map.lock().await.insert(email, (id, auth));

    Ok(())
}
