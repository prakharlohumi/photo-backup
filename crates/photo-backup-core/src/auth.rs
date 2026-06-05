use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const DEFAULT_SCOPE: &str = "https://www.googleapis.com/auth/photoslibrary.appendonly";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenCache {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_unix_secs: Option<u64>,
}

impl TokenCache {
    pub fn is_expired(&self) -> bool {
        match self.expires_at_unix_secs {
            Some(expiry) => now_unix_secs() + 60 >= expiry,
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GoogleAuthConfig {
    pub client_id: String,
    pub client_secret: Option<String>,
    pub token_cache_path: PathBuf,
}

pub fn load_or_authorize(
    config: &GoogleAuthConfig,
    stop_requested: &Arc<AtomicBool>,
) -> anyhow::Result<TokenCache> {
    if let Some(token) = load_token(&config.token_cache_path)? {
        if !token.is_expired() {
            return Ok(token);
        }
        if let Some(refresh_token) = token.refresh_token.clone() {
            if let Ok(refreshed) = refresh_access_token(config, &refresh_token) {
                save_token(&config.token_cache_path, &refreshed)?;
                return Ok(refreshed);
            }
        }
    }

    let token = interactive_login_with_stop(config, stop_requested)?;
    save_token(&config.token_cache_path, &token)?;
    Ok(token)
}

pub fn refresh_access_token(
    config: &GoogleAuthConfig,
    refresh_token: &str,
) -> anyhow::Result<TokenCache> {
    let client = http_client()?;
    let mut form = vec![
        ("client_id", config.client_id.clone()),
        ("grant_type", String::from("refresh_token")),
        ("refresh_token", refresh_token.to_string()),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()?
        .error_for_status()?;
    let body: TokenResponse = response.json()?;
    Ok(TokenCache {
        access_token: body.access_token,
        refresh_token: Some(refresh_token.to_string()),
        expires_at_unix_secs: body
            .expires_in
            .map(|secs| now_unix_secs().saturating_add(secs)),
    })
}

pub fn interactive_login_with_stop(
    config: &GoogleAuthConfig,
    stop_requested: &Arc<AtomicBool>,
) -> anyhow::Result<TokenCache> {
    let verifier = random_verifier();
    let challenge = pkce_challenge(&verifier);
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    let state = random_state();

    let auth_url = build_auth_url(&config.client_id, &redirect_uri, &challenge, &state);
    if webbrowser::open(&auth_url).is_err() {
        println!("Open this URL in your browser:\n{auth_url}");
    }

    let (code, returned_state) = receive_code(listener, stop_requested)?;
    if returned_state != state {
        anyhow::bail!("OAuth state mismatch");
    }

    let client = http_client()?;
    let mut form = vec![
        ("client_id", config.client_id.clone()),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", String::from("authorization_code")),
        ("redirect_uri", redirect_uri),
    ];
    if let Some(secret) = &config.client_secret {
        form.push(("client_secret", secret.clone()));
    }
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()?
        .error_for_status()?;
    let body: TokenResponse = response.json()?;
    Ok(TokenCache {
        access_token: body.access_token,
        refresh_token: body.refresh_token,
        expires_at_unix_secs: body
            .expires_in
            .map(|secs| now_unix_secs().saturating_add(secs)),
    })
}

fn receive_code(
    listener: TcpListener,
    stop_requested: &Arc<AtomicBool>,
) -> anyhow::Result<(String, String)> {
    loop {
        if stop_requested.load(Ordering::SeqCst) {
            anyhow::bail!("authorization cancelled");
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let mut buffer = [0u8; 4096];
                let bytes = stream.read(&mut buffer)?;
                let request = String::from_utf8_lossy(&buffer[..bytes]);
                let request_line = request.lines().next().context("missing request line")?;
                let mut parts = request_line.split_whitespace();
                let _method = parts.next().context("missing method")?;
                let path = parts.next().context("missing path")?;
                let response = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nYou can return to the backup app.";
                stream.write_all(response.as_bytes())?;

                let url = url::Url::parse(&format!("http://localhost{path}"))?;
                let code = url
                    .query_pairs()
                    .find(|(key, _)| key == "code")
                    .map(|(_, value)| value.to_string())
                    .context("missing authorization code")?;
                let state = url
                    .query_pairs()
                    .find(|(key, _)| key == "state")
                    .map(|(_, value)| value.to_string())
                    .context("missing oauth state")?;
                return Ok((code, state));
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(200));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

fn build_auth_url(client_id: &str, redirect_uri: &str, challenge: &str, state: &str) -> String {
    let scope = urlencoding::encode(DEFAULT_SCOPE);
    format!(
        "{AUTH_URL}?client_id={}&redirect_uri={}&response_type=code&scope={scope}&access_type=offline&prompt=consent&include_granted_scopes=true&code_challenge={challenge}&code_challenge_method=S256&state={state}",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
    )
}

fn random_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn random_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn http_client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .build()?)
}

#[derive(Debug, Clone, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
}

fn token_cache_exists(path: &Path) -> bool {
    path.exists()
}

fn load_token(path: &Path) -> anyhow::Result<Option<TokenCache>> {
    if !token_cache_exists(path) {
        return Ok(None);
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let token = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(token))
}

fn save_token(path: &Path, token: &TokenCache) -> anyhow::Result<()> {
    let parent = path.parent().context("missing token cache parent")?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(".google_token.json.tmp");
    fs::write(&temp, serde_json::to_vec_pretty(token)?)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn sleep_briefly() {
    thread::sleep(Duration::from_millis(200));
}
