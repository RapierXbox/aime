use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, LazyLock, OnceLock},
    time::{Duration, SystemTime, SystemTimeError},
};

use ::tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::Mutex,
};
use google_gmail1::api::Profile;
use log::{error, info};
use oauth2::{basic::BasicClient, EndpointSet, RefreshToken};
use oauth2::{reqwest, EndpointNotSet};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, CsrfToken, PkceCodeChallenge, RedirectUrl, Scope,
    TokenResponse, TokenUrl,
};
use serde::Deserialize;
use tauri::{AppHandle, Url};
use tauri_plugin_keyring::KeyringExt;

use crate::{
    email::repo::{AccountConfig, EmailAccount},
    KEYRING_SERVICE,
};

static CLIENT_SECRET_STR: &str = include_str!("../../../google_client_secret.json");
#[derive(Deserialize, Debug, Clone)]
struct ClientSecret {
    pub installed: OAuthConfig,
}

#[derive(Deserialize, Debug, Clone)]
struct OAuthConfig {
    pub client_id: String,
    pub project_id: String,
    pub auth_uri: String,
    pub token_uri: String,
    // TODO: remove
    pub auth_provider_x509_cert_url: String,
    pub client_secret: String,
    pub redirect_uris: Vec<String>,
}

static CLIENT_SECRET: LazyLock<OAuthConfig> = LazyLock::new(|| {
    serde_json::from_slice::<ClientSecret>(CLIENT_SECRET_STR.as_bytes())
        .unwrap()
        .installed
});

#[derive(Clone, Debug)]
struct TokenInfo {
    pub access_token: String,
    pub refresh_token: RefreshToken,
    pub expires_on: SystemTime,
    // todo: check if scopes are needed
    pub scopes: Vec<String>,
    // from yup_oauth2::TokenInfo, not needed?
    // pub id_token: Option<String>,
}

#[derive(Clone, Debug)]
/// This does not do any refreshing, used for getting user Data before instantiating `Auth`
pub struct TempAuth {
    token: TokenInfo,
    client: OAuthClient,
    http_client: reqwest::Client,
}

type GetTokenOutput<'a> = Pin<
    Box<
        dyn Future<Output = Result<Option<String>, Box<dyn std::error::Error + Send + Sync>>>
            + Send
            + 'a,
    >,
>;

impl google_gmail1::common::GetToken for TempAuth {
    fn get_token<'a>(&'a self, _scopes: &'a [&str]) -> GetTokenOutput<'a> {
        Box::pin(async move { Ok(Some(self.token.access_token.clone())) })
    }
}

impl TempAuth {
    pub async fn do_oauth2_flow(http_client: reqwest::Client) -> Result<Self, crate::Error> {
        oauth2_flow(http_client).await
    }

    pub fn promote(self, profile: Profile) -> Auth {
        Auth {
            inner: Arc::new(Inner {
                token: Mutex::new(self.token),
                profile,
                oauth_client: self.client,
                http_client: self.http_client,
            }),
        }
    }

    pub fn scopes(&self) -> &[String] {
        &self.token.scopes
    }

    /// Refresh tokens are stored in the system keyring
    /// The service is defined by [`KEYRING_SERVICE`]
    /// The key is `gmail:refresh:{email_addr}`
    pub fn save_to_keyring(
        &self,
        app: &tauri::AppHandle,
        email_addr: &str,
    ) -> Result<(), crate::Error> {
        app.keyring()
            .set_password(
                KEYRING_SERVICE,
                &fmt_keyring_usr(email_addr),
                self.token.refresh_token.secret(),
            )
            .inspect_err(|e| info!("could not save {:?} to keyring: {:?}", email_addr, e))
            .map_err(|_| crate::Error::KeyringSaveError)
    }
}

/// Get the keyring key/username for a given email address. Should store the refresh token.
fn fmt_keyring_usr(email_addr: &str) -> String {
    format!("gmail:refresh:{}", email_addr)
}

#[derive(Debug, Clone)]
pub struct Auth {
    // Auth internally refreshes the token when it expires,
    // sharing the same auth state across all threads
    inner: Arc<Inner>,
}

/// Manages Authorization state
/// Persists the refresh token in the keyring
#[derive(Debug)]
struct Inner {
    // Mutex since the access token may change
    token: Mutex<TokenInfo>,
    profile: Profile,
    oauth_client: OAuthClient,
    http_client: reqwest::Client,
}

impl google_gmail1::common::GetToken for Auth {
    fn get_token<'a>(&'a self, _scopes: &'a [&str]) -> GetTokenOutput<'a> {
        // TODO: check for scope match
        Box::pin(async move {
            self.ensure_refresh().await?;
            let token = self.inner.token.lock().await;
            Ok(Some(token.access_token.clone()))
        })
    }
}

impl Auth {
    /// Ensures refreshed credentials are available
    async fn ensure_refresh(&self) -> Result<(), crate::Error> {
        self.inner.enshure_refresh().await
    }

    // todo: this api should use typestate to ensure the account is a gmail account
    pub fn reinstantiate(
        app: &AppHandle,
        account: EmailAccount,
        http_client: reqwest::Client,
    ) -> Result<Self, crate::Error> {
        // load refresh token from keyring
        let res = app
            .keyring()
            .get_password(KEYRING_SERVICE, &fmt_keyring_usr(&account.email));
        let refresh_token = match res {
            Ok(Some(token)) => RefreshToken::new(token),
            Ok(None) | Err(_) => {
                info!("could not load {:?} from keyring: {:?}", account, res);
                return Err(crate::Error::KeyringLoadError);
            }
        };

        // TODO: remove in favor of typestate
        let (history_id, scopes) = match account.account_config {
            AccountConfig::Gmail { history_id, scopes } => (history_id, scopes),
        };

        let s = Self {
            inner: Arc::new(Inner {
                token: Mutex::new(TokenInfo {
                    // empty since access_token is not persisted
                    access_token: String::new(),
                    refresh_token,
                    // always refresh the access token on first use
                    expires_on: SystemTime::UNIX_EPOCH,
                    scopes,
                }),
                profile: Profile {
                    email_address: Some(account.email),
                    history_id: Some(history_id),
                    // TODO: check if needed to be set
                    messages_total: None,
                    threads_total: None,
                },
                oauth_client: create_oauth2_client(None)?,
                http_client,
            }),
        };

        Ok(s)
    }
}

impl Inner {
    /// Ensures refreshed credentials
    async fn enshure_refresh(&self) -> Result<(), crate::Error> {
        let mut token = self.token.lock().await;

        // if the expiry date is within than 60 seconds from now, refresh the token
        // get the elapsed time since expiry
        match SystemTime::now().duration_since(token.expires_on) {
            // expiry has passed, refresh token
            Ok(_) => self.refresh(&mut token).await,

            // errors if token.expires_on is in the future
            // this err contains the duration until expiry
            // if it's <= 60s, still refresh the token
            Err(err) if err.duration() <= Duration::from_secs(60) => self.refresh(&mut token).await,

            // expiry is not within 60s, do nothing
            _ => Ok(()),
        }
    }

    async fn refresh(&self, token: &mut TokenInfo) -> Result<(), crate::Error> {
        let res = self
            .oauth_client
            .exchange_refresh_token(&token.refresh_token)
            .request_async(&self.http_client)
            .await
            .expect("TODO refresh token failed");
        let new_refresh_token = res.refresh_token().unwrap().clone();

        token.refresh_token = new_refresh_token;
        Ok(())
    }
}

type OAuthClient = BasicClient<
    EndpointSet,    // HasAuthUrl
    EndpointNotSet, // HasDeviceAuthUrl
    EndpointNotSet, // HasIntrospectionUrl
    EndpointNotSet, // HasRevocationUrl
    EndpointSet,    // HasTokenUrl
>;

/// **PKCE Flow with localhost redirect**
///
/// From [https://developers.google.com/identity/protocols/oauth2/native-app]
/// TODO:
/// - Errors from [https://developers.google.com/identity/protocols/oauth2/native-app#authorization-errors]
/// - implement Dpop [https://developers.google.com/identity/protocols/oauth2/native-app#constructing-dpop-proof]
async fn oauth2_flow(http_client: reqwest::Client) -> Result<TempAuth, crate::Error> {
    // listen for the local redirect on an ephemeral port

    let tcp_listener = TcpListener::bind("127.0.0.1:0").await?;
    let local_addr = tcp_listener.local_addr()?;

    let client = create_oauth2_client(Some(local_addr))?;

    // generate a PKCE Code challenge
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

    let requested_scope = Scope::new("https://www.googleapis.com/auth/gmail.modify".to_string());

    // Generate the full authorization URL.
    let (auth_url, csrf_token) = client
        .authorize_url(CsrfToken::new_random)
        // Set the desired scopes
        .add_scope(requested_scope)
        .set_pkce_challenge(pkce_challenge)
        .url();

    // Open the authorization URL in the user's browser.
    open::that(auth_url.as_str())?;

    // Wait for the user to complete the authorization process.
    let (stream, _) = tcp_listener.accept().await?;

    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    // Read the first line of the response
    reader.read_line(&mut buf).await?;
    println!("got={}", buf);
    // respond with a 200 OK
    reader
        .into_inner() // respond with a small html page
        .write_all(b"HTTP/1.1 200 OK\nContent-Type: text/html\nContent-Length: 57\n\n<html><body><h1>You may return to aime</h1></body></html>")
        .await?;

    // now in buf: "GET /?state=...&iss=...&code=...&scope=... HTTP/1.1"
    let endpoint = buf
        .split_whitespace()
        .nth(1)
        .expect("TODO http redirect wrong form");

    let url_str = format!("http://localhost{endpoint}");
    let url = Url::parse(&url_str).expect("TODO redirect URL wrong form, parse error");
    let query_params = url.query_pairs().collect::<HashMap<_, _>>();
    let state = query_params
        .get("state")
        .map(|s| CsrfToken::new(s.to_string()))
        .expect("TODO Redirect URL missing state param");

    let code = query_params
        .get("code")
        .map(|c| AuthorizationCode::new(c.to_string()))
        .expect("TODO Redirect URL missing code param");

    // this uses feature `timing-resistant-secret-traits` to compare `state` with `csrf_token` in a timing-resistant way
    if state != csrf_token {
        panic!("client csrf token mismatch");
    }

    let c = client
        .exchange_code(code)
        .set_pkce_verifier(pkce_verifier)
        .request_async(&http_client)
        .await
        .map_err(|e| {
            error!("{e}");
            crate::Error::OAuth
        })
        .expect("token response should be ok");

    let access_token = c.access_token().clone();
    let refresh_token = c
        .refresh_token()
        .expect("refresh token should be present")
        .clone();
    let expires_on = SystemTime::now() + c.expires_in().unwrap_or(Duration::new(3600, 0));

    // TODO: check that all scopes were granted
    let scopes = c
        .scopes()
        .unwrap()
        .iter()
        .map(|it| it.to_string())
        .collect();

    Ok(TempAuth {
        token: TokenInfo {
            access_token: access_token.into_secret(),
            refresh_token,
            expires_on,
            scopes,
            // todo: check if id_token gets returned by google
            // id_token: None,
        },
        client,
        http_client,
    })
}

// this is the OAuth2 client used for Gmail authentication
// TODO: this listens to the same http port every time. when we need to do reauth, maybe we should
// keep the TcpListener on the same addr => store it?
// since its ephemeral right now, we need a different solution eventually
fn create_oauth2_client(
    local_redirect_addr: Option<std::net::SocketAddr>,
) -> Result<OAuthClient, crate::Error> {
    let client =
        oauth2::basic::BasicClient::new(ClientId::new(CLIENT_SECRET.client_id.to_string()))
            .set_auth_uri(AuthUrl::new(CLIENT_SECRET.auth_uri.to_string())?)
            .set_token_uri(TokenUrl::new(CLIENT_SECRET.token_uri.to_string())?)
            .set_client_secret(oauth2::ClientSecret::new(
                CLIENT_SECRET.client_secret.to_string(),
            ));
    // Set the URL the user will be redirected to after the authorization process.

    match local_redirect_addr {
        Some(addr) => {
            let client = client.set_redirect_uri(RedirectUrl::new(format!("http://{addr}"))?);
            Ok(client)
        }
        None => Ok(client),
    }
}
