//! Read-through: give me this key, or compute it and store it — and the
//! stampede question, which is the whole reason this file is long.
//!
//! # The problem
//!
//! A key that costs two seconds to compute expires. In the next two seconds a
//! thousand requests miss it. Without a single-flight mechanism all thousand
//! run the computation: a thousand copies of the expensive query, arriving at
//! the moment the cache stopped protecting the thing behind it. The usual
//! outcome is that the database falls over and the cache never gets written at
//! all, so the next thousand requests do it again.
//!
//! Postgres offers two real answers. They are not close.
//!
//! # Answer one: advisory locks, which this crate does not use
//!
//! `pg_advisory_lock(hashtext($key))` gives the first caller the lock and
//! blocks the rest until it is released. It is the obvious answer and it is
//! the wrong one here, for a reason that is specific rather than stylistic:
//!
//! **A session advisory lock is held by a connection, and a waiter is a
//! connection too.** Holding the lock across a two-second computation holds
//! one pool connection for two seconds doing nothing. The nine hundred and
//! ninety-nine waiters each hold another one, blocked inside the database.
//! A pool has — what, twenty connections? — so the stampede does not hit the
//! expensive query any more; it hits the connection pool, and everything else
//! in the application that wanted a connection fails with it. A mechanism that
//! converts "one query runs a thousand times" into "every request in the
//! process fails" has made the incident worse.
//!
//! Two smaller objections, in case the first one were survivable: the lock key
//! is a 64-bit integer, so a cache key has to be hashed into it and two
//! unrelated keys can collide and serialise on each other; and the advisory
//! lock space is per-database, not per-application, so two applications
//! sharing a Postgres share it.
//!
//! `pg_try_advisory_lock` avoids the blocking, but then the losers have
//! nothing to wait on and you are back to writing the poll loop below — with
//! the collisions still there.
//!
//! # Answer two: a lease, which is what this does
//!
//! `INSERT … ON CONFLICT … DO UPDATE … WHERE the-existing-row-is-expired …
//! RETURNING 1` is an atomic compare-and-set against the primary key. The
//! first caller to miss writes a row with **no value and a short expiry** —
//! a *lease* — and the statement commits immediately. No lock is held, no
//! connection is held, and the computation runs outside the database
//! entirely.
//!
//! The losers get no row back, so they know somebody else is computing. They
//! poll [`Cache::get`] with a backing-off interval until the value appears.
//!
//! What this costs, stated plainly:
//!
//! * **The losers poll.** A thousand waiters polling a primary-key lookup at
//!   20 ms, backing off to 200 ms, over two seconds, is on the order of ten
//!   thousand index lookups. That is cheap compared to one copy of the thing
//!   they are waiting for, let alone a thousand, but it is not free, and it is
//!   the price of not holding a connection.
//! * **A holder that dies leaves the lease.** It expires —
//!   [`FetchPolicy::lease_for`], thirty seconds by default — and the next
//!   caller takes over. Until then, callers that run out of
//!   [`FetchPolicy::wait_for`] compute anyway, so nobody is blocked by a dead
//!   process; they just lose the deduplication.
//! * **It is not exactly once.** It is *approximately* once: a lease that
//!   expires mid-computation, or a clock skewed between two instances, lets a
//!   second caller start. For a cache that is the right trade — the failure is
//!   a duplicated computation, never a wrong answer — and anything stronger is
//!   a distributed lock, which is a different problem with a different crate.
//!
//! # Why `fetch` does not swallow cache errors
//!
//! If the cache cannot be reached, `fetch` returns the error rather than
//! quietly computing and carrying on. For a Redis-backed cache that would be
//! the wrong call: Redis is a second system, it can be down while the
//! application is healthy, and degrading to "just compute it" is exactly what
//! you want.
//!
//! Here it is not a second system. The cache is in the same Postgres that the
//! computation is almost certainly about to query. A request that cannot reach
//! the cache is a request that is going to fail two lines later anyway, and
//! hiding the first failure only moves the error message somewhere less
//! useful. This is one of the places where putting the cache in the database
//! makes the design *simpler* than the alternative: there is no
//! partial-availability mode to think about.
//!
//! # Why `fetch` fails when the computed value is too large
//!
//! Because the alternative is worse. If `fetch` returned the value and skipped
//! the store, every later call would recompute it, which is the stampede you
//! called `fetch` to avoid, and nothing would say so. Failing is loud, and it
//! is found in development. A caller that genuinely cannot bound the value
//! should check [`Cache::max_value_bytes`] and not use the cache for it.

use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, Instant};

use crate::cache::Cache;
use crate::error::CacheError;
use crate::key::Key;
use crate::tag::Tag;

/// A boxed future, as `async_trait` would make.
///
/// Written out here because [`CacheExt::fetch`] is generic over a closure and
/// its future, which is the one case `async_trait` does not express cleanly.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How hard [`CacheExt::fetch`] tries to avoid computing twice.
///
/// The defaults suit a computation that takes well under a second, which is
/// most of them. Raise [`FetchPolicy::lease_for`] and
/// [`FetchPolicy::wait_for`] together for a slow one: a lease shorter than the
/// computation lets a second caller start, and a wait shorter than the
/// computation makes every waiter give up and compute.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FetchPolicy {
    /// How long the computed value lives.
    pub ttl: Duration,
    /// How long the claim on the key lasts.
    ///
    /// This is the ceiling on how long a crashed computer blocks the
    /// deduplication, so it should be a little longer than the computation
    /// ever takes and no longer. Default: thirty seconds.
    pub lease_for: Duration,
    /// How long a caller that lost the race waits before computing anyway.
    ///
    /// A cache must not be the reason a request hangs. When this runs out the
    /// waiter computes the value itself, which is the stampede's own behaviour
    /// — but for one request rather than all of them. Default: five seconds.
    pub wait_for: Duration,
    /// How long a waiter sleeps before its first re-read. Default: 20 ms.
    pub poll_every: Duration,
    /// The longest a waiter sleeps between re-reads, after backing off.
    ///
    /// The interval doubles from [`FetchPolicy::poll_every`] up to this, which
    /// keeps a short computation's waiters responsive and a long one's cheap.
    /// Default: 200 ms.
    pub poll_at_most: Duration,
    /// The tags the computed value is stored with. Empty by default, which
    /// stores it with a plain [`Cache::set`].
    pub tags: Vec<Tag>,
}

impl FetchPolicy {
    /// The defaults, for a value that should live `ttl`.
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            lease_for: Duration::from_secs(30),
            wait_for: Duration::from_secs(5),
            poll_every: Duration::from_millis(20),
            poll_at_most: Duration::from_millis(200),
            tags: Vec::new(),
        }
    }

    /// Stores the computed value with these tags. See [`crate::tag`].
    #[must_use]
    pub fn tags(mut self, tags: impl IntoIterator<Item = Tag>) -> Self {
        self.tags = tags.into_iter().collect();
        self
    }

    /// How long the claim on the key lasts.
    #[must_use]
    pub fn lease_for(mut self, lease_for: Duration) -> Self {
        self.lease_for = lease_for;
        self
    }

    /// How long a caller that lost the race waits before computing anyway.
    #[must_use]
    pub fn wait_for(mut self, wait_for: Duration) -> Self {
        self.wait_for = wait_for;
        self
    }

    /// The first and the longest poll interval for a waiter.
    #[must_use]
    pub fn polling(mut self, every: Duration, at_most: Duration) -> Self {
        self.poll_every = every;
        self.poll_at_most = at_most;
        self
    }
}

/// A [`CacheExt::fetch`] that did not produce a value.
///
/// Two very different things, kept apart because a caller nearly always wants
/// to treat them differently: the cache failed, or your own computation did.
#[derive(Debug)]
pub enum FetchError<E> {
    /// The cache itself failed. See [`crate::fetch`] on why this is not
    /// swallowed.
    Cache(CacheError),
    /// The computation returned an error. It was not stored and the lease was
    /// given back, so the next caller starts immediately.
    Compute(E),
}

impl<E> FetchError<E> {
    /// The computation's error, if that is what this is.
    pub fn into_compute(self) -> Option<E> {
        match self {
            FetchError::Compute(e) => Some(e),
            FetchError::Cache(_) => None,
        }
    }

    /// The cache's error, if that is what this is.
    pub fn into_cache(self) -> Option<CacheError> {
        match self {
            FetchError::Cache(e) => Some(e),
            FetchError::Compute(_) => None,
        }
    }
}

impl<E> From<CacheError> for FetchError<E> {
    fn from(error: CacheError) -> Self {
        FetchError::Cache(error)
    }
}

impl<E: std::fmt::Display> std::fmt::Display for FetchError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Cache(e) => write!(f, "{e}"),
            FetchError::Compute(e) => write!(f, "computing a cache entry: {e}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for FetchError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Cache(e) => Some(e),
            FetchError::Compute(e) => Some(e),
        }
    }
}

/// Read-through caching, for every [`Cache`] including `dyn Cache`.
///
/// Separate from [`Cache`] because [`CacheExt::fetch`] is generic over the
/// closure that computes the value, and a generic method would make the trait
/// not object-safe. The blanket implementation covers `?Sized`, so
/// `Arc<dyn Cache>` gets it too.
pub trait CacheExt: Cache {
    /// The value at `key`, computing and storing it if it is not there.
    ///
    /// Exactly one caller computes, under the single-flight lease described in
    /// [`crate::fetch`]; the rest wait and read what it stored.
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use brocanth_cache::{Cache, CacheExt, Namespace, PgCache};
    /// # async fn example(cache: &PgCache) -> Result<(), Box<dyn std::error::Error>> {
    /// let views = Namespace::new("views")?;
    /// let key = views.key("home")?;
    ///
    /// let page = cache
    ///     .fetch(&key, Duration::from_secs(60), || async {
    ///         render_the_home_page().await
    ///     })
    ///     .await?;
    /// # let _ = page;
    /// # Ok(())
    /// # }
    /// # async fn render_the_home_page() -> Result<Vec<u8>, std::io::Error> { Ok(Vec::new()) }
    /// ```
    ///
    /// # Errors
    /// [`FetchError::Compute`] when `compute` failed — the lease is released
    /// first, so the next caller starts at once. [`FetchError::Cache`] when
    /// the cache failed, including [`CacheError::TooLarge`] for a computed
    /// value over the cap.
    fn fetch<'a, F, Fut, E>(
        &'a self,
        key: &'a Key,
        ttl: Duration,
        compute: F,
    ) -> BoxFuture<'a, std::result::Result<Vec<u8>, FetchError<E>>>
    where
        F: FnOnce() -> Fut + Send + 'a,
        Fut: Future<Output = std::result::Result<Vec<u8>, E>> + Send + 'a,
        E: Send + 'a,
    {
        self.fetch_with(key, FetchPolicy::new(ttl), compute)
    }

    /// [`CacheExt::fetch`], with the single-flight behaviour spelled out.
    ///
    /// # Errors
    /// As [`CacheExt::fetch`].
    fn fetch_with<'a, F, Fut, E>(
        &'a self,
        key: &'a Key,
        policy: FetchPolicy,
        compute: F,
    ) -> BoxFuture<'a, std::result::Result<Vec<u8>, FetchError<E>>>
    where
        F: FnOnce() -> Fut + Send + 'a,
        Fut: Future<Output = std::result::Result<Vec<u8>, E>> + Send + 'a,
        E: Send + 'a,
    {
        Box::pin(async move {
            // The hit, which is the case that has to be one round trip.
            if let Some(value) = self.get(key).await? {
                return Ok(value);
            }

            // The race. Exactly one `lease` answers true; everyone else waits
            // for that one to store, and gives up waiting rather than hanging.
            let deadline = Instant::now() + policy.wait_for;
            let mut interval = policy.poll_every;
            let mut holding_lease = false;
            loop {
                if self.lease(key, policy.lease_for).await? {
                    holding_lease = true;
                    break;
                }
                if let Some(value) = self.get(key).await? {
                    return Ok(value);
                }
                let now = Instant::now();
                if now >= deadline {
                    // The holder is slower than we are willing to wait, or it
                    // died and its lease has not expired yet. Compute it
                    // ourselves: one duplicated computation beats a request
                    // that hangs on a cache.
                    break;
                }
                tokio::time::sleep(interval.min(deadline - now)).await;
                interval = (interval * 2).min(policy.poll_at_most);
            }

            match compute().await {
                Ok(value) => match store(self, key, &value, &policy).await {
                    Ok(()) => Ok(value),
                    Err(error) => {
                        if holding_lease {
                            // Best effort. The lease expires on its own if
                            // this fails too, and failing here would replace
                            // the error that matters.
                            let _ = self.release_lease(key).await;
                        }
                        Err(FetchError::Cache(error))
                    }
                },
                Err(error) => {
                    if holding_lease {
                        let _ = self.release_lease(key).await;
                    }
                    Err(FetchError::Compute(error))
                }
            }
        })
    }

    /// A rendered fragment of a page: the cached text at `key`, or `render`'s
    /// output, stored with `tags` for `ttl`.
    ///
    /// `render` is a plain closure, not a future, because a template renders
    /// synchronously: put the template call inside it. It runs at most once,
    /// and only on a miss, under the same single-flight lease as
    /// [`CacheExt::fetch`].
    ///
    /// ```no_run
    /// # use std::time::Duration;
    /// # use brocanth_cache::{CacheExt, Namespace, PgCache, Tag};
    /// # async fn example(cache: &PgCache) -> Result<(), Box<dyn std::error::Error>> {
    /// let fragments = Namespace::new("fragments")?;
    /// let sidebar = cache
    ///     .fragment(
    ///         &fragments.key("sidebar")?,
    ///         &[Tag::table("posts")?],
    ///         Duration::from_secs(300),
    ///         || render_sidebar(),
    ///     )
    ///     .await?;
    /// # let _ = sidebar;
    /// # Ok(())
    /// # }
    /// # fn render_sidebar() -> String { String::new() }
    /// ```
    ///
    /// # Errors
    /// [`CacheError`] when the cache failed, for the reason [`crate::fetch`]
    /// gives for not hiding it.
    fn fragment<'a, F>(
        &'a self,
        key: &'a Key,
        tags: &'a [Tag],
        ttl: Duration,
        render: F,
    ) -> BoxFuture<'a, std::result::Result<String, CacheError>>
    where
        F: FnOnce() -> String + Send + 'a,
    {
        Box::pin(async move {
            self.try_fragment(key, tags, ttl, || {
                Ok::<_, std::convert::Infallible>(render())
            })
            .await
            .map_err(|error| match error {
                FetchError::Cache(error) => error,
                FetchError::Compute(never) => match never {},
            })
        })
    }

    /// [`CacheExt::fragment`], for a render that can fail. A failed render is
    /// not stored.
    ///
    /// # Errors
    /// [`FetchError::Compute`] when `render` failed; [`FetchError::Cache`]
    /// when the cache did.
    fn try_fragment<'a, F, E>(
        &'a self,
        key: &'a Key,
        tags: &'a [Tag],
        ttl: Duration,
        render: F,
    ) -> BoxFuture<'a, std::result::Result<String, FetchError<E>>>
    where
        F: FnOnce() -> std::result::Result<String, E> + Send + 'a,
        E: Send + 'a,
    {
        let policy = FetchPolicy::new(ttl).tags(tags.iter().cloned());
        Box::pin(async move {
            let bytes = self
                .fetch_with(key, policy, || {
                    let rendered = render().map(String::into_bytes);
                    std::future::ready(rendered)
                })
                .await?;
            // Only this method stores text under a fragment key, so the bytes
            // are UTF-8. A key shared with a byte value by mistake reads as
            // replacement characters, not as a panic.
            Ok(String::from_utf8(bytes)
                .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
        })
    }
}

impl<C: Cache + ?Sized> CacheExt for C {}

/// Stores a computed value, with its tags when it has any.
async fn store<C: Cache + ?Sized>(
    cache: &C,
    key: &Key,
    value: &[u8],
    policy: &FetchPolicy,
) -> crate::error::Result<()> {
    if policy.tags.is_empty() {
        cache.set(key, value, policy.ttl).await
    } else {
        cache.set_tagged(key, value, policy.ttl, &policy.tags).await
    }
}
