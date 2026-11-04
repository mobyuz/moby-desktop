//! Client for moby-api. All traffic and all tokens stay on this side of the IPC
//! boundary: the refresh token lives in the Windows Credential Manager, the access
//! token in memory, and the webview only ever sees account data.

use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, Method, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::async_runtime::Mutex;
use tauri::{AppHandle, Manager, State};

use crate::agent::Agent;
use crate::store::{Store, is_account_id};

const DEFAULT_API_URL: &str = "http://127.0.0.1:8080";
const TERMS_VERSION: &str = match option_env!("MOBY_TERMS_VERSION") {
    Some(version) => version,
    None => "dev-1",
};
const PRIVACY_POLICY_VERSION: &str = match option_env!("MOBY_PRIVACY_POLICY_VERSION") {
    Some(version) => version,
    None => "dev-1",
};

const VAULT_SERVICE: &str = "uz.moby.desktop";
const REFRESH_TOKEN: &str = "refresh-token";
const INSTALLATION_ID: &str = "installation-id";

#[derive(Debug, Serialize)]
pub struct Failure {
    code: String,
    message: String,
}

impl Failure {
    pub(crate) fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    #[cfg(test)]
    pub(crate) fn code(&self) -> &str {
        &self.code
    }

    fn offline() -> Self {
        Self::new(
            "offline",
            "Can't connect to Moby. Check your connection and try again.",
        )
    }

    fn signed_out() -> Self {
        Self::new("signed_out", "Your session ended. Sign in again.")
    }

    fn vault() -> Self {
        Self::new("vault", "Windows Credential Manager is not available.")
    }

    fn unexpected() -> Self {
        Self::new("unexpected", "Something went wrong. Try again.")
    }
}

#[derive(Deserialize)]
struct ErrorBody {
    code: String,
    message: String,
}

#[derive(Deserialize)]
struct TokenPair {
    access_token: String,
    refresh_token: String,
    installation_id: String,
}

fn vault_entry(name: &str) -> Result<keyring::Entry, Failure> {
    keyring::Entry::new(VAULT_SERVICE, name).map_err(|_| Failure::vault())
}

fn vault_get(name: &str) -> Result<Option<String>, Failure> {
    match vault_entry(name)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => Err(Failure::vault()),
    }
}

fn vault_set(name: &str, value: &str) -> Result<(), Failure> {
    vault_entry(name)?
        .set_password(value)
        .map_err(|_| Failure::vault())
}

fn vault_clear(name: &str) -> Result<(), Failure> {
    match vault_entry(name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err(Failure::vault()),
    }
}

/// Tokens only travel over HTTPS, except to a local development server.
fn parse_base_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    (url.scheme() == "https" || (url.scheme() == "http" && local))
        .then(|| value.trim_end_matches('/').to_owned())
}

/// Takes every complete SSE frame off the buffer and returns its data.
pub(crate) fn drain_frames(buffer: &mut String) -> Vec<String> {
    let mut frames = Vec::new();
    while let Some(end) = buffer.find("\n\n") {
        let frame: String = buffer.drain(..end + 2).collect();
        let data: Vec<&str> = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .collect();
        if !data.is_empty() {
            frames.push(data.join("\n"));
        }
    }
    frames
}

/// The account id in a profile, if it is safe to use as a key for local history.
fn account_of(profile: &Value) -> Option<String> {
    profile["id"]
        .as_str()
        .filter(|id| is_account_id(id))
        .map(str::to_owned)
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36 && value.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

async fn failure_from(response: Response) -> Failure {
    match response.json::<ErrorBody>().await {
        Ok(body) => Failure {
            code: body.code,
            message: body.message,
        },
        Err(_) => Failure::unexpected(),
    }
}

async fn read_json(response: Response) -> Result<Value, Failure> {
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    response.json().await.map_err(|_| Failure::unexpected())
}

pub struct Api {
    http: Client,
    base: String,
    /// Also serialises refreshes: a reused refresh token revokes the whole session.
    access: Mutex<Option<String>>,
    /// Id of the signed-in account. Local history is filed under it, and the webview cannot change it.
    account: StdMutex<Option<String>>,
    /// Changes whenever the signed-in account changes or signs out. Approvals and session grants
    /// record it, so none of them survives a sign-out.
    epoch: AtomicU64,
}

impl Api {
    pub fn new() -> Self {
        let configured = std::env::var("MOBY_API_URL").ok();
        let value = configured
            .as_deref()
            .or(option_env!("MOBY_API_URL"))
            .unwrap_or(DEFAULT_API_URL);
        Self::new_with(value)
    }

    fn new_with(value: &str) -> Self {
        let base =
            parse_base_url(value).expect("MOBY_API_URL must be https, or http on this computer");
        let http = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .build()
            .expect("failed to start the HTTP client");
        Self {
            http,
            base,
            access: Mutex::new(None),
            account: StdMutex::new(None),
            epoch: AtomicU64::new(1),
        }
    }

    pub(crate) fn set_account(&self, account: Option<String>) {
        let mut current = self.account.lock().unwrap_or_else(|e| e.into_inner());
        if *current != account {
            self.epoch.fetch_add(1, Ordering::SeqCst);
        }
        *current = account;
    }

    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
    }

    /// A client that is already signed in, for tests that talk to a stub server.
    #[cfg(test)]
    pub(crate) fn signed_in_for_test(base: &str, account: &str) -> Self {
        let api = Self::new_with(base);
        api.set_account(Some(account.to_owned()));
        *api.access.try_lock().expect("fresh client") = Some("test-token".to_owned());
        api
    }

    /// Remembers who is signed in, from the profile the server returned.
    fn remember(&self, profile: &Value) -> Result<(), Failure> {
        let id = account_of(profile).ok_or_else(Failure::unexpected)?;
        self.set_account(Some(id));
        Ok(())
    }

    pub(crate) fn account_id(&self) -> Result<String, Failure> {
        self.account
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .ok_or_else(Failure::signed_out)
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, format!("{}{path}", self.base))
    }

    fn adopt(&self, access: &mut Option<String>, tokens: TokenPair) -> Result<(), Failure> {
        vault_set(REFRESH_TOKEN, &tokens.refresh_token)?;
        vault_set(INSTALLATION_ID, &tokens.installation_id)?;
        *access = Some(tokens.access_token);
        Ok(())
    }

    /// Swaps the refresh token for new tokens, unless another call already replaced `stale`.
    async fn refresh(&self, stale: Option<&str>) -> Result<String, Failure> {
        let mut access = self.access.lock().await;
        if let Some(current) = access.as_deref()
            && Some(current) != stale
        {
            return Ok(current.to_owned());
        }
        *access = None;
        let refresh_token = vault_get(REFRESH_TOKEN)?.ok_or_else(Failure::signed_out)?;
        let response = self
            .request(Method::POST, "/v1/auth/refresh")
            .json(&json!({ "refresh_token": refresh_token }))
            .send()
            .await
            .map_err(|_| Failure::offline())?;
        match response.status() {
            StatusCode::OK => {
                let tokens: TokenPair = response.json().await.map_err(|_| Failure::unexpected())?;
                let token = tokens.access_token.clone();
                self.adopt(&mut access, tokens)?;
                Ok(token)
            }
            StatusCode::UNAUTHORIZED => {
                self.set_account(None);
                vault_clear(REFRESH_TOKEN)?;
                Err(Failure::signed_out())
            }
            _ => Err(failure_from(response).await),
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Response, Failure> {
        let attempt = |token: String| {
            let mut request = self.request(method.clone(), path).bearer_auth(token);
            if let Some(body) = body {
                request = request.json(body);
            }
            request.send()
        };
        let current = self.access.lock().await.clone();
        let token = match current {
            Some(token) => token,
            None => self.refresh(None).await?,
        };
        let response = attempt(token.clone())
            .await
            .map_err(|_| Failure::offline())?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let token = self.refresh(Some(&token)).await?;
        let response = attempt(token).await.map_err(|_| Failure::offline())?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.forget_session().await?;
            return Err(Failure::signed_out());
        }
        Ok(response)
    }

    async fn forget_session(&self) -> Result<(), Failure> {
        self.set_account(None);
        *self.access.lock().await = None;
        vault_clear(REFRESH_TOKEN)
    }

    async fn login(
        &self,
        email: &str,
        password: &str,
        installation: Option<&str>,
    ) -> Result<Response, Failure> {
        self.request(Method::POST, "/v1/auth/login")
            .json(&json!({ "email": email, "password": password, "installation_id": installation }))
            .send()
            .await
            .map_err(|_| Failure::offline())
    }
}

#[tauri::command]
pub async fn restore_session(api: State<'_, Api>) -> Result<Option<Value>, Failure> {
    if vault_get(REFRESH_TOKEN)?.is_none() {
        return Ok(None);
    }
    match api.send(Method::GET, "/v1/me", None).await {
        Ok(response) => {
            let profile = read_json(response).await?;
            api.remember(&profile)?;
            Ok(Some(profile))
        }
        Err(failure) if failure.code == "signed_out" => Ok(None),
        Err(failure) => Err(failure),
    }
}

#[tauri::command]
pub async fn sign_in(
    api: State<'_, Api>,
    email: String,
    password: String,
) -> Result<Value, Failure> {
    let known = vault_get(INSTALLATION_ID)?;
    let mut response = api.login(&email, &password, known.as_deref()).await?;
    // The saved computer may be revoked or belong to another account. The API answers both like a wrong password.
    if response.status() == StatusCode::UNAUTHORIZED && known.is_some() {
        response = api.login(&email, &password, None).await?;
    }
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    let tokens: TokenPair = response.json().await.map_err(|_| Failure::unexpected())?;
    let mut access = api.access.lock().await;
    api.adopt(&mut access, tokens)?;
    drop(access);
    let profile = read_json(api.send(Method::GET, "/v1/me", None).await?).await?;
    api.remember(&profile)?;
    Ok(profile)
}

#[tauri::command]
pub async fn register(api: State<'_, Api>, email: String, password: String) -> Result<(), Failure> {
    let response = api
        .request(Method::POST, "/v1/auth/register")
        .json(&json!({
            "email": email,
            "password": password,
            "terms_version": TERMS_VERSION,
            "privacy_policy_version": PRIVACY_POLICY_VERSION,
        }))
        .send()
        .await
        .map_err(|_| Failure::offline())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(failure_from(response).await)
    }
}

#[tauri::command]
pub async fn forgot_password(api: State<'_, Api>, email: String) -> Result<(), Failure> {
    let response = api
        .request(Method::POST, "/v1/auth/password/forgot")
        .json(&json!({ "email": email }))
        .send()
        .await
        .map_err(|_| Failure::offline())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(failure_from(response).await)
    }
}

#[tauri::command]
pub async fn sign_out(api: State<'_, Api>, agent: State<'_, Agent>) -> Result<(), Failure> {
    agent.end_session();
    if let Some(refresh_token) = vault_get(REFRESH_TOKEN)? {
        // Signing out locally still works offline; the server session then expires on its own.
        let _ = api
            .request(Method::POST, "/v1/auth/logout")
            .json(&json!({ "refresh_token": refresh_token }))
            .send()
            .await;
    }
    api.forget_session().await
}

#[tauri::command]
pub async fn get_usage(api: State<'_, Api>) -> Result<Value, Failure> {
    read_json(api.send(Method::GET, "/v1/usage", None).await?).await
}

#[tauri::command]
pub async fn list_installations(api: State<'_, Api>) -> Result<Value, Failure> {
    read_json(api.send(Method::GET, "/v1/installations", None).await?).await
}

#[tauri::command]
pub async fn revoke_installation(api: State<'_, Api>, id: String) -> Result<(), Failure> {
    if !is_uuid(&id) {
        return Err(Failure::unexpected());
    }
    let response = api
        .send(Method::DELETE, &format!("/v1/installations/{id}"), None)
        .await?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(failure_from(response).await)
    }
}

#[tauri::command]
pub async fn update_profile(
    api: State<'_, Api>,
    display_name: Option<String>,
) -> Result<Value, Failure> {
    let body = json!({ "display_name": display_name });
    read_json(api.send(Method::PATCH, "/v1/me", Some(&body)).await?).await
}

/// Refreshes first and then sends once: here a 401 means a wrong password, not an expired token.
#[tauri::command]
pub async fn delete_account(
    api: State<'_, Api>,
    store: State<'_, Store>,
    agent: State<'_, Agent>,
    password: String,
) -> Result<(), Failure> {
    let account = api.account_id()?;
    let check = api.send(Method::GET, "/v1/me", None).await?;
    if !check.status().is_success() {
        return Err(failure_from(check).await);
    }
    let token = api
        .access
        .lock()
        .await
        .clone()
        .ok_or_else(Failure::signed_out)?;
    let response = api
        .request(Method::DELETE, "/v1/me")
        .bearer_auth(token)
        .json(&json!({ "password": password }))
        .send()
        .await
        .map_err(|_| Failure::offline())?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err(Failure::new("wrong_password", "Password is incorrect."));
    }
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    // The server has confirmed, so this account's history, registered folders, grants, reminders
    // and activity go too. The session ends either way.
    agent.end_session();
    let cleaned = store.with(|db| db.delete_account_data(&account));
    api.forget_session().await?;
    cleaned.map_err(|_| {
        Failure::new(
            "history_not_deleted",
            "Your account was deleted, but Moby couldn't remove its data from this computer. Use Settings to delete all history.",
        )
    })
}

fn free_export_path(folder: PathBuf) -> PathBuf {
    let first = folder.join("moby-account-export.json");
    if !first.exists() {
        return first;
    }
    (1..)
        .map(|n| folder.join(format!("moby-account-export-{n}.json")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Saves the export to the Downloads folder and returns where it went.
#[tauri::command]
pub async fn export_account(app: AppHandle, api: State<'_, Api>) -> Result<String, Failure> {
    let response = api.send(Method::POST, "/v1/me/export", None).await?;
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    let bytes = response.bytes().await.map_err(|_| Failure::offline())?;
    let folder = app
        .path()
        .download_dir()
        .map_err(|_| Failure::unexpected())?;
    let path = free_export_path(folder);
    std::fs::write(&path, bytes)
        .map_err(|_| Failure::new("save_failed", "Could not save the file to Downloads."))?;
    Ok(path.display().to_string())
}

/// Opens the event stream for one agent request. Used by `agent.rs`, which owns the turn.
pub(crate) async fn open_agent_stream(api: &Api, request: &Value) -> Result<Response, Failure> {
    let response = api
        .send(Method::POST, "/v1/agent/turn", Some(request))
        .await?;
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    Ok(response)
}

/// Splits a response body into server-sent event payloads.
pub(crate) struct Frames {
    stream: futures_util::stream::BoxStream<'static, Result<Vec<u8>, ()>>,
    buffer: String,
    pending: Vec<u8>,
    ready: std::collections::VecDeque<String>,
}

impl Frames {
    pub(crate) fn new(response: Response) -> Self {
        Self {
            stream: response
                .bytes_stream()
                .map(|chunk| chunk.map(|bytes| bytes.to_vec()).map_err(|_| ()))
                .boxed(),
            buffer: String::new(),
            pending: Vec::new(),
            ready: std::collections::VecDeque::new(),
        }
    }

    /// The next payload, or `None` when the server closed the stream.
    pub(crate) async fn next(&mut self) -> Result<Option<String>, Failure> {
        loop {
            if let Some(frame) = self.ready.pop_front() {
                return Ok(Some(frame));
            }
            let Some(chunk) = self.stream.next().await else {
                return Ok(None);
            };
            self.pending
                .extend_from_slice(&chunk.map_err(|_| Failure::offline())?);
            // A chunk can end inside a multi-byte character, so only complete text moves on.
            let valid = match std::str::from_utf8(&self.pending) {
                Ok(text) => text.len(),
                Err(error) => error.valid_up_to(),
            };
            self.buffer.push_str(
                std::str::from_utf8(&self.pending[..valid]).map_err(|_| Failure::unexpected())?,
            );
            self.pending.drain(..valid);
            self.ready.extend(drain_frames(&mut self.buffer));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allows_https_or_a_local_server() {
        assert_eq!(
            parse_base_url("https://api.moby.uz/").as_deref(),
            Some("https://api.moby.uz")
        );
        assert_eq!(
            parse_base_url("http://127.0.0.1:8080").as_deref(),
            Some("http://127.0.0.1:8080")
        );
        assert_eq!(
            parse_base_url("http://localhost:8080").as_deref(),
            Some("http://localhost:8080")
        );
        assert_eq!(parse_base_url("http://api.moby.uz"), None);
        assert_eq!(parse_base_url("not a url"), None);
    }

    #[test]
    fn drains_complete_frames_and_keeps_the_rest() {
        let mut buffer = "data: {\"a\":1}\n\ndata:{\"b\":2}\n\ndata: {\"c\"".to_owned();
        assert_eq!(drain_frames(&mut buffer), vec!["{\"a\":1}", "{\"b\":2}"]);
        assert_eq!(buffer, "data: {\"c\"");
    }

    #[test]
    fn ignores_frames_without_data() {
        let mut buffer = ": keep-alive\n\nevent: x\ndata: 1\n\n".to_owned();
        assert_eq!(drain_frames(&mut buffer), vec!["1"]);
    }

    #[test]
    fn files_history_only_under_a_plain_account_id() {
        let id = "7f3a90c2-1111-4222-8333-444455556666";
        assert_eq!(account_of(&json!({ "id": id })).as_deref(), Some(id));
        assert_eq!(account_of(&json!({ "email": "a@b.c" })), None);
        assert_eq!(account_of(&json!({ "id": "a/../b" })), None);
        assert_eq!(account_of(&json!({ "id": 7 })), None);
    }

    #[test]
    fn rejects_ids_that_could_change_the_path() {
        assert!(is_uuid("7f3a90c2-1111-4222-8333-444455556666"));
        assert!(!is_uuid("../me"));
        assert!(!is_uuid("7f3a90c2/1111-4222-8333-444455556666"));
    }
}
