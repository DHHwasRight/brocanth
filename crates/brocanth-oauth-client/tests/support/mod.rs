//! A fake provider on a local port, and a clock a test moves. No network
//! beyond loopback.
//!
//! The provider checks what a real one checks: the client credentials, the
//! PKCE verifier against the challenge of the code, the redirect URI, and
//! that a refresh token is one it issued and has not voided. It rotates
//! refresh tokens: each refresh voids the one it was given.

#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use brocanth_oauth_client::web::{ConnectAnswer, ConnectRequest, ConnectRoutes};
use brocanth_oauth_client::{
    Clock, Endpoint, Key, MemoryTokenStore, OauthClient, Provider, TokenStore,
};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const CLIENT_ID: &str = "cid";
pub const SECRET: &str = "the-client-secret";
pub const BASE: &str = "https://app.example/connections";

/// A clock a test moves.
#[derive(Default)]
pub struct Manual(pub AtomicI64);

impl Manual {
    pub fn at(now: i64) -> Arc<Self> {
        Arc::new(Self(AtomicI64::new(now)))
    }
    pub fn advance(&self, seconds: i64) {
        self.0.fetch_add(seconds, Ordering::SeqCst);
    }
}

impl Clock for Manual {
    fn now(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct Fake {
    pub url: String,
    /// code -> (PKCE challenge, redirect URI)
    codes: Mutex<HashMap<String, String>>,
    pub refresh: Mutex<HashSet<String>>,
    pub access: Mutex<HashSet<String>>,
    next: AtomicU64,
    pub refreshes: AtomicU64,
    pub app_grants: AtomicU64,
    pub revoked: Mutex<Vec<String>>,
    pub expires_in: AtomicI64,
    /// Milliseconds a refresh takes, so concurrent callers overlap.
    pub delay_ms: AtomicU64,
    /// Answer every API call 401.
    pub refuse_api: std::sync::atomic::AtomicBool,
    /// Give no refresh token on the code exchange.
    pub no_refresh: std::sync::atomic::AtomicBool,
}

impl Fake {
    pub async fn start() -> Arc<Fake> {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let fake = Arc::new(Fake {
            url: format!("http://{}", listener.local_addr().unwrap()),
            expires_in: AtomicI64::new(3600),
            ..Fake::default()
        });
        let served = Arc::clone(&fake);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let fake = Arc::clone(&served);
                tokio::spawn(async move {
                    let Some((head, body)) = read_request(&mut socket).await else {
                        return;
                    };
                    let (status, text) = fake.answer(&head, &body).await;
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                        text.len()
                    );
                    let _ = socket.write_all(reply.as_bytes()).await;
                    let _ = socket.shutdown().await;
                });
            }
        });
        fake
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint::new(
            format!("{}/authorize", self.url),
            format!("{}/token", self.url),
        )
        .revoke_url(format!("{}/revoke", self.url))
    }

    pub fn provider(&self) -> Provider {
        Provider::new("fake", "Fake", self.endpoint(), CLIENT_ID, SECRET)
            .scopes(["read", "write"])
            .app_scopes(["app"])
    }

    /// The person consents: the provider issues `code` for this challenge.
    pub fn consent(&self, code: &str, challenge: &str) {
        self.codes
            .lock()
            .unwrap()
            .insert(code.to_string(), challenge.to_string());
    }

    fn issue(&self, prefix: &str) -> String {
        format!("{prefix}-{}", self.next.fetch_add(1, Ordering::SeqCst) + 1)
    }

    fn tokens(&self, refresh: bool) -> String {
        let access = self.issue("at");
        self.access.lock().unwrap().insert(access.clone());
        let expires = self.expires_in.load(Ordering::SeqCst);
        if refresh {
            let rt = self.issue("rt");
            self.refresh.lock().unwrap().insert(rt.clone());
            format!(
                r#"{{"access_token":"{access}","token_type":"bearer","expires_in":{expires},"refresh_token":"{rt}"}}"#
            )
        } else {
            format!(r#"{{"access_token":"{access}","token_type":"bearer","expires_in":{expires}}}"#)
        }
    }

    async fn answer(&self, head: &str, body: &str) -> (u16, String) {
        let line = head.lines().next().unwrap_or_default();
        let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
            .into_owned()
            .collect();
        let bad = |error: &str| (400, format!(r#"{{"error":"{error}"}}"#));
        if line.starts_with("GET /api") {
            let bearer = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("authorization")
                        .then(|| v.trim().to_string())
                })
                .unwrap_or_default();
            let ok = !self.refuse_api.load(Ordering::SeqCst)
                && bearer
                    .strip_prefix("Bearer ")
                    .is_some_and(|t| self.access.lock().unwrap().contains(t));
            return if ok {
                (200, r#"{"ok":true}"#.to_string())
            } else {
                (401, r#"{"error":"invalid_token"}"#.to_string())
            };
        }
        if line.starts_with("POST /revoke") {
            self.revoked
                .lock()
                .unwrap()
                .push(form.get("token").cloned().unwrap_or_default());
            return (200, "{}".to_string());
        }
        if !line.starts_with("POST /token") {
            return (404, "{}".to_string());
        }
        if form.get("client_id").map(String::as_str) != Some(CLIENT_ID)
            || form.get("client_secret").map(String::as_str) != Some(SECRET)
        {
            return (401, r#"{"error":"invalid_client"}"#.to_string());
        }
        match form.get("grant_type").map(String::as_str) {
            Some("authorization_code") => {
                let code = form.get("code").cloned().unwrap_or_default();
                let Some(challenge) = self.codes.lock().unwrap().remove(&code) else {
                    return bad("invalid_grant");
                };
                let verifier = form.get("code_verifier").cloned().unwrap_or_default();
                if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) != challenge {
                    return bad("invalid_grant");
                }
                if form.get("redirect_uri").map(String::as_str)
                    != Some(&format!("{BASE}/fake/callback"))
                {
                    return bad("invalid_grant");
                }
                (200, self.tokens(!self.no_refresh.load(Ordering::SeqCst)))
            }
            Some("refresh_token") => {
                let given = form.get("refresh_token").cloned().unwrap_or_default();
                let delay = self.delay_ms.load(Ordering::SeqCst);
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                // Rotation: the token given is void from now on.
                if !self.refresh.lock().unwrap().remove(&given) {
                    return bad("invalid_grant");
                }
                self.refreshes.fetch_add(1, Ordering::SeqCst);
                (200, self.tokens(true))
            }
            Some("client_credentials") => {
                if form.get("scope").map(String::as_str) != Some("app") {
                    return bad("invalid_scope");
                }
                self.app_grants.fetch_add(1, Ordering::SeqCst);
                let delay = self.delay_ms.load(Ordering::SeqCst);
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                (200, self.tokens(false))
            }
            _ => bad("unsupported_grant_type"),
        }
    }
}

async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<(String, String)> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buf.len() < end + length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[end..]).into_owned();
    Some((head, body))
}

/// A client over the fake, a memory store and a manual clock.
pub fn client(fake: &Fake, clock: Arc<Manual>, store: Arc<dyn TokenStore>) -> OauthClient {
    OauthClient::builder(Key::from_bytes([9; 32]))
        .store(store)
        .provider(fake.provider())
        .callback_base(BASE)
        .allow_http(true)
        .clock(clock)
        .build()
        .expect("the client builds")
}

pub fn memory() -> Arc<dyn TokenStore> {
    Arc::new(MemoryTokenStore::new())
}

/// One query parameter of a URL.
pub fn param(url: &str, name: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.into_owned())
}

/// What the browser holds after a start: the consent URL and the cookie.
pub struct Begun {
    pub url: String,
    pub cookie: String,
    pub state: String,
}

/// Posts the start form as `account`.
pub async fn start(routes: &ConnectRoutes, account: &str, body: &str) -> Begun {
    let answer = routes
        .answer(ConnectRequest {
            post: true,
            path: "/connections/fake",
            body,
            account: Some(account),
            ..ConnectRequest::default()
        })
        .await
        .expect("the start answers");
    let ConnectAnswer::Redirect { location, cookies } = answer else {
        panic!("a redirect to the provider, got {answer:?}");
    };
    let set = &cookies[0];
    assert!(
        set.contains("HttpOnly") && set.contains("SameSite=Lax"),
        "{set}"
    );
    let cookie = set.split(';').next().unwrap().to_string();
    let state = param(&location, "state").expect("a state");
    Begun {
        url: location,
        cookie,
        state,
    }
}

/// The provider's redirect back, as a callback request.
pub async fn callback(
    routes: &ConnectRoutes,
    cookies: &str,
    query: &str,
    account: Option<&str>,
) -> ConnectAnswer {
    let path = "/connections/fake/callback";
    routes
        .answer(ConnectRequest {
            path,
            query,
            cookies,
            account,
            ..ConnectRequest::default()
        })
        .await
        .expect("the callback answers")
}

/// Connects `account` all the way: start, consent, callback.
pub async fn connect(fake: &Fake, routes: &ConnectRoutes, account: &str) {
    let begun = start(routes, account, "").await;
    let challenge = param(&begun.url, "code_challenge").unwrap();
    let code = format!("code-for-{account}");
    fake.consent(&code, &challenge);
    let answer = callback(
        routes,
        &begun.cookie,
        &format!("code={code}&state={}", begun.state),
        None,
    )
    .await;
    let ConnectAnswer::Page { status: 200, .. } = answer else {
        panic!("connected, got {answer:?}");
    };
}
