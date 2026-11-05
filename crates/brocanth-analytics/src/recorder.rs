//! [`Recorder`]: the buffer between the hook and the store.
//!
//! The hook runs inside a request and must not wait, so it only pushes a
//! [`Pending`] onto a buffer. A task drains the buffer every few seconds.
//! A write that fails puts the batch back and is retried; the request that
//! made the view was answered long before.

use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::day::Day;
use crate::store::{Report, Store, StoreError, View};

/// The most views held in memory. Past it, new views are dropped and
/// counted in [`Recorder::dropped`], so a database that is down for an hour
/// cannot grow the process without bound.
pub const MAX_BUFFER: usize = 50_000;

/// A page view before its salt is known.
///
/// The hook hashes the address and the user agent at once, so neither ever
/// sits in the buffer. The day's salt is applied at write time, by the
/// flusher, which can wait for the store.
#[derive(Clone, Debug)]
pub struct Pending {
    /// The site the view belongs to: the recorder's own, or the host of a
    /// static site that sent a beacon.
    pub site: String,
    /// The UTC day of the request, fixed when it arrived.
    pub day: Day,
    /// `SHA-256(site ‖ 0 ‖ address ‖ 0 ‖ user agent)`.
    pub prehash: [u8; 32],
    /// The cleaned path.
    pub path: String,
    /// The allowed query parameters.
    pub query: String,
    /// The referrer origin, or empty.
    pub referrer: String,
    /// The browser family.
    pub browser: &'static str,
    /// The system family.
    pub os: &'static str,
    /// The device kind.
    pub device: &'static str,
    /// The country code, or empty.
    pub country: String,
}

/// A clock, so a test can move the day.
pub type Clock = Arc<dyn Fn() -> SystemTime + Send + Sync>;

/// The shared half of the analytics: the buffer, the store, the clock.
///
/// Cheap to clone. The hook holds one, the flusher another, the admin
/// panel a third.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Inner>,
}

struct Inner {
    site: String,
    retention_days: u32,
    store: OnceLock<Arc<dyn Store>>,
    buffer: Mutex<Vec<Pending>>,
    dropped: AtomicU64,
    written: AtomicU64,
    last_error: Mutex<Option<String>>,
    /// The day [`Store::forget`] last ran, so it runs once a day.
    swept: AtomicI32,
    clock: Clock,
    /// One flush at a time, so the timer and a shutdown never write one
    /// batch twice.
    flushing: tokio::sync::Mutex<()>,
}

impl Recorder {
    /// A recorder for `site` with no store yet: views wait in the buffer
    /// until [`Recorder::attach`] gives it one.
    pub fn new(site: impl Into<String>, retention_days: u32, clock: Clock) -> Self {
        Self {
            inner: Arc::new(Inner {
                site: site.into(),
                retention_days,
                store: OnceLock::new(),
                buffer: Mutex::new(Vec::new()),
                dropped: AtomicU64::new(0),
                written: AtomicU64::new(0),
                last_error: Mutex::new(None),
                swept: AtomicI32::new(i32::MIN),
                clock,
                flushing: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Gives the recorder its store. The first call wins; a second one is
    /// ignored and answers `false`.
    pub fn attach(&self, store: Arc<dyn Store>) -> bool {
        self.inner.store.set(store).is_ok()
    }

    /// The store, once attached.
    pub fn store(&self) -> Option<&Arc<dyn Store>> {
        self.inner.store.get()
    }

    /// The site name every row carries.
    pub fn site(&self) -> &str {
        &self.inner.site
    }

    /// Today, by this recorder's clock.
    pub fn today(&self) -> Day {
        Day::of((self.inner.clock)())
    }

    /// The prehash of one visitor of `site`: their address and user agent,
    /// mixed with the site and never stored. The site is in the hash, so one
    /// person on two sites is two unrelated visitors.
    pub fn prehash(&self, site: &str, address: &str, user_agent: &str) -> [u8; 32] {
        let mut digest = Sha256::new();
        digest.update(site.as_bytes());
        digest.update([0]);
        digest.update(address.as_bytes());
        digest.update([0]);
        digest.update(user_agent.as_bytes());
        to_array(&digest.finalize())
    }

    /// Queues one view. Never waits and never fails: past [`MAX_BUFFER`],
    /// the view is dropped and counted.
    pub fn record(&self, pending: Pending) {
        let mut buffer = self.buffer();
        if buffer.len() >= MAX_BUFFER {
            self.inner.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        buffer.push(pending);
    }

    /// Views waiting to be written.
    pub fn pending(&self) -> usize {
        self.buffer().len()
    }

    /// Views dropped because the buffer was full.
    pub fn dropped(&self) -> u64 {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Views written since the process started.
    pub fn written(&self) -> u64 {
        self.inner.written.load(Ordering::Relaxed)
    }

    /// The last write error, cleared by the next write that succeeds.
    pub fn last_error(&self) -> Option<String> {
        self.inner
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Writes the buffer to the store, then, once a day, deletes old salts
    /// and rows past the retention.
    ///
    /// Answers the number of views written. With no store attached it
    /// writes nothing and keeps the buffer.
    ///
    /// # Errors
    /// The store's error. The batch goes back on the buffer, so the next
    /// flush tries it again.
    pub async fn flush(&self) -> Result<usize, StoreError> {
        let _one = self.inner.flushing.lock().await;
        let Some(store) = self.inner.store.get().cloned() else {
            return Ok(0);
        };
        let batch = std::mem::take(&mut *self.buffer());
        let written = match self.write(&*store, &batch).await {
            Ok(n) => n,
            Err(e) => {
                self.requeue(batch);
                self.fail(&e);
                return Err(e);
            }
        };
        let today = self.today();
        if self.inner.swept.swap(today.0, Ordering::SeqCst) != today.0 {
            let keep_from = match self.inner.retention_days {
                0 => None,
                days => Some(today.minus(days)),
            };
            if let Err(e) = store.forget(today, keep_from).await {
                // Try again on the next flush rather than tomorrow.
                self.inner.swept.store(i32::MIN, Ordering::SeqCst);
                self.fail(&e);
                return Err(e);
            }
        }
        *self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        Ok(written)
    }

    /// The report for this recorder's own site, from the store.
    ///
    /// # Errors
    /// No store attached, or the store's error.
    pub async fn report(&self, from: Day, to: Day) -> Result<Report, StoreError> {
        let site = self.inner.site.clone();
        self.report_for(&site, from, to).await
    }

    /// The report for any site with rows: this one, or a static site that
    /// sends beacons.
    ///
    /// # Errors
    /// No store attached, or the store's error.
    pub async fn report_for(&self, site: &str, from: Day, to: Day) -> Result<Report, StoreError> {
        self.attached()?.report(site, from, to).await
    }

    /// Every site with rows, this recorder's own first, whether or not it
    /// has any yet.
    ///
    /// # Errors
    /// No store attached, or the store's error.
    pub async fn sites(&self) -> Result<Vec<String>, StoreError> {
        let mut sites = vec![self.inner.site.clone()];
        for site in self.attached()?.sites().await? {
            if !sites.contains(&site) {
                sites.push(site);
            }
        }
        Ok(sites)
    }

    fn attached(&self) -> Result<&Arc<dyn Store>, StoreError> {
        self.inner
            .store
            .get()
            .ok_or_else(|| StoreError::new("the analytics store is not attached yet"))
    }

    async fn write(&self, store: &dyn Store, batch: &[Pending]) -> Result<usize, StoreError> {
        if batch.is_empty() {
            return Ok(0);
        }
        let mut salts: Vec<(Day, [u8; 32])> = Vec::new();
        let mut views = Vec::with_capacity(batch.len());
        for pending in batch {
            let salt = match salts.iter().find(|(day, _)| *day == pending.day) {
                Some((_, salt)) => *salt,
                None => {
                    let salt = store.salt(pending.day).await?;
                    salts.push((pending.day, salt));
                    salt
                }
            };
            views.push(View {
                site: pending.site.clone(),
                day: pending.day,
                visitor: visitor(&salt, &pending.prehash),
                path: pending.path.clone(),
                query: pending.query.clone(),
                referrer: pending.referrer.clone(),
                browser: pending.browser.to_string(),
                os: pending.os.to_string(),
                device: pending.device.to_string(),
                country: pending.country.clone(),
            });
        }
        store.write(&views).await?;
        self.inner
            .written
            .fetch_add(views.len() as u64, Ordering::Relaxed);
        Ok(views.len())
    }

    fn requeue(&self, batch: Vec<Pending>) {
        let mut buffer = self.buffer();
        let room = MAX_BUFFER.saturating_sub(buffer.len());
        let lost = batch.len().saturating_sub(room);
        let mut batch = batch;
        batch.truncate(room);
        // The failed batch is older than what arrived meanwhile.
        batch.append(&mut buffer);
        *buffer = batch;
        self.inner.dropped.fetch_add(lost as u64, Ordering::Relaxed);
    }

    fn fail(&self, error: &StoreError) {
        *self
            .inner
            .last_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error.to_string());
    }

    fn buffer(&self) -> std::sync::MutexGuard<'_, Vec<Pending>> {
        self.inner
            .buffer
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The stored visitor: the first 8 bytes of `SHA-256(salt ‖ prehash)`.
pub fn visitor(salt: &[u8; 32], prehash: &[u8; 32]) -> i64 {
    let mut digest = Sha256::new();
    digest.update(salt);
    digest.update(prehash);
    let out = to_array(&digest.finalize());
    let mut first = [0u8; 8];
    first.copy_from_slice(&out[..8]);
    i64::from_be_bytes(first)
}

/// 32 bytes from the OS CSPRNG.
///
/// # Errors
/// The OS could not supply them.
pub fn fresh_salt() -> Result<[u8; 32], StoreError> {
    let mut salt = [0u8; 32];
    getrandom::fill(&mut salt).map_err(|e| StoreError::new(format!("no randomness: {e}")))?;
    Ok(salt)
}

fn to_array(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[..32]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_prehash_under_two_salts_gives_two_visitors() {
        let prehash = [7u8; 32];
        let a = visitor(&[1u8; 32], &prehash);
        assert_eq!(a, visitor(&[1u8; 32], &prehash));
        assert_ne!(a, visitor(&[2u8; 32], &prehash));
    }

    #[test]
    fn two_salts_are_never_equal() {
        assert_ne!(fresh_salt().unwrap(), fresh_salt().unwrap());
    }
}
