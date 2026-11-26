//! The application's half of the wire: a signed `POST` in, a signed JSON
//! answer out (W1, W4, W5, W6).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};
use std::time::{Duration, Instant};

use brocanth::routing::Verb;
use brocanth_admin::data::SharedData;
use brocanth_admin::panel::remote::PanelServe;
use brocanth_server::{Request, Response};

use super::sign::{now, random_hex, ServiceSecret};
use super::{
    Ask, Replied, Reply, NONCE_HEADER, PATH, SIGNATURE_HEADER, TIME_HEADER, TX_MAX, TX_TIMEOUT,
    VERSION, WINDOW,
};
use crate::contract::{Answer, Caller, Claim, ErrorKind, TabData, TabError, TabTransaction};
use crate::server::InProcess;

/// The most nonces the server remembers. Past it, a request waits for the
/// window to forget one: refused with 503, never let through.
const NONCES: usize = 65_536;

/// The application's half of the wire.
///
/// Its data side is `InProcess::enforcing_grants()` over the source it is
/// given: a server must not trust the caller's RBAC decision (D6), and
/// there is no way to turn the check off. A request is decoded only after
/// its signature verifies, so a row scope and a claim are accepted only
/// from a backoffice that holds the secret (D4).
///
/// Cheap to clone: the clones share the held transactions and the nonces.
#[derive(Clone)]
pub struct WireServer {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for WireServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WireServer")
            .field("tabs", &self.inner.tabs)
            .field("timeout", &self.inner.timeout)
            .field("max", &self.inner.max)
            .finish_non_exhaustive()
    }
}

struct Inner {
    tabs: InProcess,
    secret: ServiceSecret,
    panels: HashMap<String, Arc<dyn PanelServe>>,
    nonces: Mutex<HashMap<String, u64>>,
    held: Mutex<HashMap<String, Slot>>,
    timeout: Duration,
    max: usize,
    reaper: OnceLock<()>,
}

/// A held transaction, between two requests or inside one.
enum Slot {
    /// Waiting for its next request.
    Idle {
        tx: Box<dyn TabTransaction>,
        subject: Option<String>,
        until: Instant,
    },
    /// A request is using it, or `Begin` is opening it.
    Busy,
}

impl WireServer {
    /// The wire over `source`, the admin's data source, signed with
    /// `secret`. Transactions are held for [`TX_TIMEOUT`], at most
    /// [`TX_MAX`] at once, until [`WireServer::holding`] says else.
    pub fn new(source: SharedData, secret: ServiceSecret) -> WireServer {
        WireServer {
            inner: Arc::new(Inner {
                tabs: InProcess::new(source).enforcing_grants(),
                secret,
                panels: HashMap::new(),
                nonces: Mutex::default(),
                held: Mutex::default(),
                timeout: TX_TIMEOUT,
                max: TX_MAX,
                reaper: OnceLock::new(),
            }),
        }
    }

    fn inner_mut(&mut self) -> &mut Inner {
        Arc::get_mut(&mut self.inner).expect("a wire server is configured before it is cloned")
    }

    /// Keeps a transaction nobody uses for `timeout`, and at most `max`
    /// open at once (W6).
    ///
    /// # Panics
    ///
    /// When this server was cloned already: configure it first.
    pub fn holding(mut self, timeout: Duration, max: usize) -> WireServer {
        let inner = self.inner_mut();
        inner.timeout = timeout;
        inner.max = max;
        self
    }

    /// Serves `tab`'s panel data contract through `Ask::Panel` (§7.6).
    ///
    /// # Panics
    ///
    /// When this server was cloned already: configure it first.
    pub fn panel(mut self, tab: impl Into<String>, serve: Arc<dyn PanelServe>) -> WireServer {
        self.inner_mut().panels.insert(tab.into(), serve);
        self
    }

    /// Whether `path` is the wire's.
    pub fn answers(&self, path: &str) -> bool {
        path == PATH
    }

    /// How many transactions are open now.
    pub fn open_transactions(&self) -> usize {
        lock(&self.inner.held).len()
    }

    /// Answers one request to [`PATH`].
    pub async fn handle(&self, request: Request) -> Response {
        if request.verb != Verb::Post {
            return Response::text(405, "the backoffice wire takes POST\n")
                .with_header("allow", "POST");
        }
        let inner = &self.inner;
        inner.start_reaper();
        inner.sweep().await;

        let header = |name: &str| request.header(name).unwrap_or_default().to_string();
        let (time, nonce, signature) = (
            header(TIME_HEADER),
            header(NONCE_HEADER),
            header(SIGNATURE_HEADER),
        );
        if let Err(error) = inner.authenticate(&time, &nonce, &signature, &request) {
            return error;
        }
        let ask: Ask = match serde_json::from_slice(&request.body) {
            Ok(ask) => ask,
            Err(e) => {
                let error = TabError::protocol(format!("the wire could not decode a request: {e}"));
                return inner.reply(&nonce, 400, &Err(error));
            }
        };
        let replied = inner.answer(ask).await;
        inner.reply(&nonce, 200, &replied)
    }
}

impl Inner {
    /// Refuses a request that is not signed with the secret, is too old or
    /// too new, or was seen before. A refusal is not signed: the request
    /// proved nothing.
    fn authenticate(
        &self,
        time: &str,
        nonce: &str,
        signature: &str,
        request: &Request,
    ) -> Result<(), Response> {
        let refused = |status: u16, message: String, help: &str| {
            let error = TabError::new(ErrorKind::Refused, message, help);
            let body = serde_json::to_vec(&error).unwrap_or_default();
            Err(Response::json(status, body))
        };
        let same_secret = "give both binaries the same --backoffice.service_secret_file, \
                           and keep their clocks within a minute of each other";
        if !self
            .secret
            .verifies(time, nonce, &request.path, &request.body, signature)
        {
            return refused(
                401,
                "the backoffice wire refused a request that is not signed with the service secret"
                    .to_string(),
                same_secret,
            );
        }
        let Ok(sent) = time.parse::<u64>() else {
            return refused(
                401,
                "the request's time is not a number".to_string(),
                same_secret,
            );
        };
        let clock = now();
        if sent.abs_diff(clock) > WINDOW.as_secs() {
            return refused(
                401,
                format!(
                    "the request was signed at {sent}, {}s from this server's clock",
                    sent.abs_diff(clock)
                ),
                same_secret,
            );
        }
        let mut nonces = lock(&self.nonces);
        nonces.retain(|_, seen| seen.abs_diff(clock) <= WINDOW.as_secs());
        if nonces.contains_key(nonce) {
            return refused(
                401,
                "the backoffice wire refused a request it already answered".to_string(),
                "send each request once: a replayed request is refused",
            );
        }
        if nonces.len() >= NONCES {
            return refused(
                503,
                format!("the backoffice wire saw {NONCES} requests in the last minute"),
                "wait a minute, or send fewer requests",
            );
        }
        nonces.insert(nonce.to_string(), clock);
        Ok(())
    }

    /// The answer, as signed JSON.
    fn reply(&self, nonce: &str, status: u16, replied: &Replied) -> Response {
        let body = serde_json::to_vec(replied).unwrap_or_else(|e| {
            serde_json::to_vec(&Replied::Err(TabError::protocol(format!(
                "the wire could not encode an answer: {e}"
            ))))
            .unwrap_or_default()
        });
        let signature = self.secret.sign_reply(nonce, status, &body);
        Response::json(status, body).with_header(SIGNATURE_HEADER, &signature)
    }

    async fn answer(&self, ask: Ask) -> Replied {
        match ask {
            Ask::Hello { version } => {
                if version != VERSION {
                    return Err(TabError::protocol(format!(
                        "the backoffice speaks wire version {version}, and the application {VERSION}"
                    )));
                }
                Ok(Reply::Hello {
                    version: VERSION,
                    registry: self.tabs.registry(),
                    check: self.tabs.check().err(),
                })
            }
            Ask::Call(call) => self.tabs.call(call).await.map(Reply::Answer),
            Ask::Begin(claim) => self.begin(claim).await,
            Ask::InTx { tx, caller, write } => self.in_tx(&tx, caller, write).await,
            Ask::Commit { tx } => {
                let tx = self.take(&tx)?;
                tx.commit().await.map(|()| Reply::Answer(Answer::Done))
            }
            Ask::Rollback { tx } => {
                let tx = self.take(&tx)?;
                tx.rollback().await.map(|()| Reply::Answer(Answer::Done))
            }
            Ask::Panel { tab, claim, call } => self.panel(&tab, &claim, &call).await,
        }
    }

    async fn begin(&self, claim: Claim) -> Replied {
        let id = random_hex::<16>();
        {
            let mut held = lock(&self.held);
            if held.len() >= self.max {
                return Err(TabError::new(
                    ErrorKind::Backend,
                    format!(
                        "the backoffice wire holds {} open transactions, its most",
                        self.max
                    ),
                    "commit or roll back the open ones, wait for them to time out, or raise \
                     --backoffice.tx_max",
                ));
            }
            held.insert(id.clone(), Slot::Busy);
        }
        // A request cut off while the transaction opens frees the slot.
        let mut reserved = Reserved {
            held: &self.held,
            id: &id,
            armed: true,
        };
        let opened = self.tabs.begin(&claim).await;
        reserved.armed = false;
        drop(reserved);
        match opened {
            Ok(tx) => {
                lock(&self.held).insert(
                    id.clone(),
                    Slot::Idle {
                        tx,
                        subject: claim.subject,
                        until: Instant::now() + self.timeout,
                    },
                );
                Ok(Reply::Opened { tx: id })
            }
            Err(error) => {
                lock(&self.held).remove(&id);
                Err(error)
            }
        }
    }

    async fn in_tx(&self, id: &str, caller: Caller, write: crate::contract::Write) -> Replied {
        let tx = {
            let mut held = lock(&self.held);
            match held.remove(id) {
                None => return Err(gone(id, self.timeout)),
                Some(Slot::Busy) => {
                    held.insert(id.to_string(), Slot::Busy);
                    return Err(TabError::protocol(format!(
                        "transaction `{id}` is running another request"
                    )));
                }
                Some(Slot::Idle { tx, subject, until }) => {
                    if subject != caller.claim.subject {
                        held.insert(id.to_string(), Slot::Idle { tx, subject, until });
                        return Err(TabError::new(
                            ErrorKind::Refused,
                            format!("transaction `{id}` belongs to another member of staff"),
                            "send each write with the claim that opened its transaction",
                        ));
                    }
                    held.insert(id.to_string(), Slot::Busy);
                    (tx, subject)
                }
            }
        };
        // A request cut off here drops the transaction, which keeps
        // nothing, and frees its slot.
        let mut using = Using {
            held: &self.held,
            id,
            tx: Some(tx),
        };
        let answer = match using.tx.as_mut() {
            Some((tx, _)) => tx.call(caller, write).await,
            None => unreachable!("the transaction is taken only here"),
        };
        using.release(self.timeout);
        answer.map(Reply::Answer)
    }

    /// Takes an idle transaction out, to end it.
    fn take(&self, id: &str) -> Result<Box<dyn TabTransaction>, TabError> {
        let mut held = lock(&self.held);
        match held.remove(id) {
            None => Err(gone(id, self.timeout)),
            Some(Slot::Busy) => {
                held.insert(id.to_string(), Slot::Busy);
                Err(TabError::protocol(format!(
                    "transaction `{id}` is running another request"
                )))
            }
            Some(Slot::Idle { tx, .. }) => Ok(tx),
        }
    }

    async fn panel(&self, tab: &str, claim: &Claim, call: &str) -> Replied {
        let Some(serve) = self.panels.get(tab) else {
            return Err(TabError::new(
                ErrorKind::Protocol,
                format!("the application serves no panel data for the tab `{tab}`"),
                "register the panel's data on the wire server (`WireServer::panel`), or build \
                 both binaries from one version of the application",
            ));
        };
        let level = serve.level(call).map_err(TabError::protocol)?;
        let caller = Caller {
            tab: tab.to_string(),
            level,
            claim: claim.clone(),
        };
        // The panel says what the call needs: one level on the tab by
        // default, or its own rule (the roles tab's owners) (§7.6, P3).
        if !serve
            .allows(tab, call, &claim.grants())
            .map_err(TabError::protocol)?
        {
            return Err(TabError::refused(&caller));
        }
        match serve.serve(&claim.principal(), call).await {
            Ok(answer) => Ok(Reply::Panel(answer)),
            Err(text) => Err(TabError::from(brocanth_admin::data::DataError::Backend(
                text,
            ))),
        }
    }

    /// Rolls back every transaction idle past its time.
    async fn sweep(&self) {
        let now = Instant::now();
        let expired: Vec<Box<dyn TabTransaction>> = {
            let mut held = lock(&self.held);
            let ids: Vec<String> = held
                .iter()
                .filter(|(_, slot)| matches!(slot, Slot::Idle { until, .. } if *until <= now))
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| match held.remove(&id) {
                    Some(Slot::Idle { tx, .. }) => Some(tx),
                    _ => None,
                })
                .collect()
        };
        for tx in expired {
            // A rollback that fails still ends the transaction: it is
            // dropped, and a dropped transaction keeps nothing.
            let _ = tx.rollback().await;
        }
    }

    /// Starts the task that sweeps the held transactions every half
    /// timeout, once, on the runtime the first request runs on. It holds
    /// the server weakly, and ends with it.
    fn start_reaper(self: &Arc<Self>) {
        self.reaper.get_or_init(|| {
            let Ok(runtime) = tokio::runtime::Handle::try_current() else {
                return;
            };
            let weak: Weak<Inner> = Arc::downgrade(self);
            let every = (self.timeout / 2).max(Duration::from_millis(10));
            runtime.spawn(async move {
                loop {
                    tokio::time::sleep(every).await;
                    let Some(inner) = weak.upgrade() else {
                        break;
                    };
                    inner.sweep().await;
                }
            });
        });
    }
}

fn gone(id: &str, timeout: Duration) -> TabError {
    TabError::protocol(format!(
        "the application holds no transaction `{id}`: it ended, or it was idle for {}s and \
         rolled back",
        timeout.as_secs()
    ))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A slot `Begin` reserved: freed if the request ends before it opens.
struct Reserved<'a> {
    held: &'a Mutex<HashMap<String, Slot>>,
    id: &'a str,
    armed: bool,
}

impl Drop for Reserved<'_> {
    fn drop(&mut self) {
        if self.armed {
            lock(self.held).remove(self.id);
        }
    }
}

/// A transaction one request is using.
struct Using<'a> {
    held: &'a Mutex<HashMap<String, Slot>>,
    id: &'a str,
    tx: Option<(Box<dyn TabTransaction>, Option<String>)>,
}

impl Using<'_> {
    /// Puts it back, idle, for another `timeout`.
    fn release(&mut self, timeout: Duration) {
        if let Some((tx, subject)) = self.tx.take() {
            lock(self.held).insert(
                self.id.to_string(),
                Slot::Idle {
                    tx,
                    subject,
                    until: Instant::now() + timeout,
                },
            );
        }
    }
}

impl Drop for Using<'_> {
    fn drop(&mut self) {
        if self.tx.take().is_some() {
            lock(self.held).remove(self.id);
        }
    }
}
