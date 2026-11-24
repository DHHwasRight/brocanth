//! The backoffice's half of the wire: a [`TabData`] that sends each call
//! to the application (W7, W8).

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use brocanth_admin::discovery::RegistryEntry;
use brocanth_admin::panel::remote::PanelWire;
use brocanth_admin::policy::Principal;

use super::sign::{now, sign, ServiceSecret};
use super::{
    check_url, Ask, Replied, Reply, NONCE_HEADER, PATH, SIGNATURE_HEADER, TIME_HEADER, VERSION,
};
use crate::client::ClaimFn;
use crate::contract::{
    Answer, Call, Caller, Claim, ErrorKind, SharedTabs, TabData, TabError, TabTransaction, Write,
};

/// How long one request may take, connect included.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A [`TabData`] over HTTP: the backoffice's half of the wire.
///
/// Its first request is `Hello`, which reads the registry and the data
/// side's check once (W7). [`WireClient::connect`] sends it before it
/// returns; [`WireClient::lazy`] sends it when the admin first asks for the
/// registry or the check, on a thread of its own, because the admin builds
/// synchronously. Either way a failure is the client's
/// [`TabData::check`] error, so the admin's start-time check stops the
/// start with it.
#[derive(Clone)]
pub struct WireClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WireClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireClient")
            .field("url", &self.inner.url)
            .finish_non_exhaustive()
    }
}

struct Inner {
    /// The wire's full URL: the application's, then [`PATH`].
    url: String,
    /// The secret, or why there is none to use.
    secret: Result<ServiceSecret, TabError>,
    http: reqwest::Client,
    hello: OnceLock<Result<Hello, TabError>>,
}

/// What `Hello` answered.
#[derive(Clone)]
struct Hello {
    registry: Vec<RegistryEntry>,
    check: Option<TabError>,
}

impl WireClient {
    /// The client of the application at `base` (`http://127.0.0.1:3000`),
    /// connected: `Hello` is answered before this returns.
    ///
    /// # Errors
    ///
    /// When the application cannot be reached, refuses the secret, or
    /// speaks another version; with a `help:` line.
    pub async fn connect(base: &str, secret: ServiceSecret) -> Result<WireClient, TabError> {
        let client = WireClient::build(base, Ok(secret));
        let hello = client.inner.hello().await;
        let failed = hello.as_ref().err().cloned();
        let _ = client.inner.hello.set(hello);
        match failed {
            Some(error) => Err(error),
            None => Ok(client),
        }
    }

    /// The client of the application at `base`, with the secret as the
    /// settings hold it. Nothing is sent until the admin asks for the
    /// registry or the check; a bad URL or secret is the check's error.
    ///
    /// `plugins::backoffice::contract` builds one in the backoffice binary
    /// when `[backoffice] data_url` is set (W8).
    pub fn lazy(base: &str, secret: Option<&str>) -> WireClient {
        let secret = secret.map_or_else(
            || {
                Err(
                    "`[backoffice] data_url` is set, and `[backoffice] service_secret` is not \
                     set\nhelp: give both binaries the same secret: \
                     --backoffice.service_secret_file=PATH"
                        .to_string(),
                )
            },
            ServiceSecret::parse,
        );
        WireClient::build(base, secret.map_err(config_error))
    }

    fn build(base: &str, secret: Result<ServiceSecret, TabError>) -> WireClient {
        let secret = check_url(base).map_err(config_error).and(secret);
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .unwrap_or_default();
        WireClient {
            inner: Arc::new(Inner {
                url: format!("{}{PATH}", base.trim_end_matches('/')),
                secret,
                http,
                hello: OnceLock::new(),
            }),
        }
    }

    /// As the admin's end holds a data side: `Contracted::over(client.shared())`.
    pub fn shared(self) -> SharedTabs {
        Arc::new(self)
    }

    /// The panels' end of the wire (§7.6): each call carries the claim
    /// `claim_of` builds from the request's principal.
    pub fn panels(
        &self,
        claim_of: impl for<'p> Fn(&Principal<'p>) -> Claim + Send + Sync + 'static,
    ) -> Arc<dyn PanelWire> {
        Arc::new(WirePanels {
            inner: Arc::clone(&self.inner),
            claim_of: Arc::new(claim_of),
        })
    }

    /// `Hello`'s answer: read once, on a thread of its own when nobody
    /// connected first.
    fn hello(&self) -> Result<&Hello, TabError> {
        self.inner
            .hello
            .get_or_init(|| {
                let inner = Arc::clone(&self.inner);
                std::thread::scope(|scope| {
                    scope
                        .spawn(move || {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(|e| {
                                    TabError::protocol(format!(
                                        "the backoffice could not start a runtime to connect: {e}"
                                    ))
                                })?;
                            // A client of its own: connections a runtime
                            // opens end with it.
                            let http = reqwest::Client::builder()
                                .timeout(TIMEOUT)
                                .build()
                                .unwrap_or_default();
                            runtime.block_on(inner.hello_with(&http))
                        })
                        .join()
                        .unwrap_or_else(|_| {
                            Err(TabError::protocol(
                                "the backoffice's connect thread panicked",
                            ))
                        })
                })
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

fn config_error(message: String) -> TabError {
    let (message, help) = match message.split_once("\nhelp: ") {
        Some((message, help)) => (message.to_string(), help.to_string()),
        None => (message, "check `[backoffice]`".to_string()),
    };
    TabError::new(ErrorKind::Backend, message, help)
}

impl Inner {
    async fn hello(&self) -> Result<Hello, TabError> {
        self.hello_with(&self.http).await
    }

    async fn hello_with(&self, http: &reqwest::Client) -> Result<Hello, TabError> {
        match self
            .send_with(http, &Ask::Hello { version: VERSION })
            .await?
        {
            Reply::Hello {
                version,
                registry,
                check,
            } if version == VERSION => Ok(Hello { registry, check }),
            Reply::Hello { version, .. } => Err(TabError::protocol(format!(
                "the application speaks wire version {version}, and the backoffice {VERSION}"
            ))),
            _ => Err(TabError::protocol(
                "the application did not answer `Hello` with its registry",
            )),
        }
    }

    async fn send(&self, ask: &Ask) -> Result<Reply, TabError> {
        self.send_with(&self.http, ask).await
    }

    /// Sends one signed request and checks the signed answer.
    async fn send_with(&self, http: &reqwest::Client, ask: &Ask) -> Result<Reply, TabError> {
        let secret = self.secret.as_ref().map_err(Clone::clone)?;
        let body = serde_json::to_vec(ask)
            .map_err(|e| TabError::protocol(format!("a request did not encode: {e}")))?;
        let signed = sign(secret, PATH, &body, now());
        let unreachable = |e: reqwest::Error| {
            TabError::new(
                ErrorKind::Backend,
                format!(
                    "the backoffice could not reach the application at {}: {e}",
                    self.url
                ),
                "start the application with --backoffice.wire=true, and check \
                 --backoffice.data_url",
            )
        };
        let response = http
            .post(&self.url)
            .header("content-type", "application/json")
            .header(TIME_HEADER, &signed.time)
            .header(NONCE_HEADER, &signed.nonce)
            .header(SIGNATURE_HEADER, &signed.signature)
            .body(body)
            .send()
            .await
            .map_err(unreachable)?;
        let status = response.status().as_u16();
        let signature = response
            .headers()
            .get(SIGNATURE_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.bytes().await.map_err(unreachable)?;
        match (status, signature) {
            (200 | 400, Some(signature))
                if secret.verifies_reply(&signed.nonce, status, &bytes, &signature) =>
            {
                let replied: Replied = serde_json::from_slice(&bytes).map_err(|e| {
                    TabError::protocol(format!("an answer of the application did not decode: {e}"))
                })?;
                replied
            }
            (401 | 503, _) => Err(serde_json::from_slice::<TabError>(&bytes).unwrap_or_else(
                |_| {
                    TabError::new(
                        ErrorKind::Refused,
                        format!("the application answered {status} to the backoffice"),
                        "give both binaries the same --backoffice.service_secret_file",
                    )
                },
            )),
            (200 | 400, _) => Err(TabError::new(
                ErrorKind::Refused,
                format!(
                    "the answer from {} is not signed with the service secret",
                    self.url
                ),
                "check that --backoffice.data_url names the application, and give both \
                 binaries the same --backoffice.service_secret_file",
            )),
            (404 | 405, _) => Err(TabError::new(
                ErrorKind::Backend,
                format!("{} does not serve the backoffice wire ({status})", self.url),
                "start the application with --backoffice.wire=true, and check \
                 --backoffice.data_url",
            )),
            _ => Err(TabError::protocol(format!(
                "the application answered {status}: {}",
                String::from_utf8_lossy(&bytes)
            ))),
        }
    }
}

#[async_trait]
impl TabData for WireClient {
    fn registry(&self) -> Vec<RegistryEntry> {
        self.hello()
            .map(|hello| hello.registry.clone())
            .unwrap_or_default()
    }

    fn check(&self) -> Result<(), TabError> {
        match &self.hello()?.check {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    async fn call(&self, call: Call) -> Result<Answer, TabError> {
        let name = call.op.name();
        match self.inner.send(&Ask::Call(call)).await? {
            Reply::Answer(answer) => Ok(answer),
            _ => Err(TabError::protocol(format!(
                "the application did not answer `{name}` with an answer"
            ))),
        }
    }

    async fn begin(&self, claim: &Claim) -> Result<Box<dyn TabTransaction>, TabError> {
        match self.inner.send(&Ask::Begin(claim.clone())).await? {
            Reply::Opened { tx } => Ok(Box::new(WireTransaction {
                inner: Arc::clone(&self.inner),
                id: tx,
                open: true,
            })),
            _ => Err(TabError::protocol(
                "the application did not answer `begin` with a transaction",
            )),
        }
    }
}

/// A transaction the application holds (W6).
struct WireTransaction {
    inner: Arc<Inner>,
    id: String,
    open: bool,
}

impl WireTransaction {
    async fn end(&mut self, ask: Ask) -> Result<(), TabError> {
        self.open = false;
        match self.inner.send(&ask).await? {
            Reply::Answer(Answer::Done) => Ok(()),
            _ => Err(TabError::protocol(
                "the application did not end the transaction",
            )),
        }
    }
}

#[async_trait]
impl TabTransaction for WireTransaction {
    async fn call(&mut self, caller: Caller, write: Write) -> Result<Answer, TabError> {
        let ask = Ask::InTx {
            tx: self.id.clone(),
            caller,
            write,
        };
        match self.inner.send(&ask).await? {
            Reply::Answer(answer) => Ok(answer),
            _ => Err(TabError::protocol(
                "the application did not answer a write with an answer",
            )),
        }
    }

    async fn commit(mut self: Box<Self>) -> Result<(), TabError> {
        let tx = self.id.clone();
        self.end(Ask::Commit { tx }).await
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), TabError> {
        let tx = self.id.clone();
        self.end(Ask::Rollback { tx }).await
    }
}

impl Drop for WireTransaction {
    /// Dropped open, it asks the application to roll back, on the runtime
    /// it was dropped on. Without one, the application's timeout ends it.
    fn drop(&mut self) {
        if !self.open {
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let inner = Arc::clone(&self.inner);
            let tx = std::mem::take(&mut self.id);
            runtime.spawn(async move {
                let _ = inner.send(&Ask::Rollback { tx }).await;
            });
        }
    }
}

/// The panels' end of the wire (§7.6, P2): a [`PanelWire`] that sends
/// `Ask::Panel` with the request's claim.
pub struct WirePanels {
    inner: Arc<Inner>,
    claim_of: ClaimFn,
}

impl std::fmt::Debug for WirePanels {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WirePanels")
            .field("url", &self.inner.url)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl PanelWire for WirePanels {
    async fn send(&self, who: &Principal<'_>, tab: &str, call: String) -> Result<String, String> {
        let ask = Ask::Panel {
            tab: tab.to_string(),
            claim: (self.claim_of)(who),
            call,
        };
        match self.inner.send(&ask).await {
            Ok(Reply::Panel(answer)) => Ok(answer),
            Ok(_) => {
                Err(TabError::protocol("the application did not answer a panel call").to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}
