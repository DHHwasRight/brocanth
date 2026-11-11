//! [`RedisCache`]: the `Cache` trait over one Redis or Valkey server.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use brocanth_cache::{
    expires_at, Cache, CacheError, Key, Namespace, Result, Tag, DEFAULT_MAX_VALUE_BYTES,
};
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::{AsyncCommands, Client, Script};

/// The prefix of every key this cache writes, unless
/// [`RedisCache::with_prefix`] names another.
///
/// A prefix lets the cache share a server with other data, and lets two
/// applications share one server without meeting.
pub const DEFAULT_PREFIX: &str = "brocanth:cache";

/// The kind byte before a stored value.
const VALUE: u8 = b'v';
/// The whole content of a lease.
const LEASE: &[u8] = b"l";

/// How long one command may wait for an answer, and one connection attempt
/// for the server. A cache that hangs is worse than a miss.
const TIMEOUT: Duration = Duration::from_secs(5);

/// How many keys one `SCAN` step asks for in [`Cache::clear`].
const SCAN_COUNT: usize = 500;

/// `set`: store, and keep the old tags alive as long as the value.
///
/// A plain `set` keeps the tags of an earlier tagged write, as the trait
/// says: an invalidation then removes more than it had to, never less. The
/// tag sets expire, so they must live at least as long as this value, or an
/// invalidation would miss it and serve a stale hit.
///
/// KEYS: entry, its tag list. ARGV: value, ttl in ms, tag-set prefix.
const SET: &str = r"
local ttl = tonumber(ARGV[2])
redis.call('SET', KEYS[1], ARGV[1], 'PX', ttl)
local tags = redis.call('SMEMBERS', KEYS[2])
if #tags > 0 then
  for _, t in ipairs(tags) do
    local k = ARGV[3] .. t
    if redis.call('PTTL', k) < ttl then redis.call('PEXPIRE', k, ttl) end
  end
  if redis.call('PTTL', KEYS[2]) < ttl then redis.call('PEXPIRE', KEYS[2], ttl) end
end
return 1
";

/// `set_tagged`: store, drop the old tags, add the new ones.
///
/// KEYS: entry, its tag list. ARGV: value, ttl in ms, tag-set prefix, then
/// the tags.
const SET_TAGGED: &str = r"
local ttl = tonumber(ARGV[2])
redis.call('SET', KEYS[1], ARGV[1], 'PX', ttl)
for _, t in ipairs(redis.call('SMEMBERS', KEYS[2])) do
  redis.call('SREM', ARGV[3] .. t, KEYS[1])
end
redis.call('DEL', KEYS[2])
for i = 4, #ARGV do
  local k = ARGV[3] .. ARGV[i]
  redis.call('SADD', k, KEYS[1])
  if redis.call('PTTL', k) < ttl then redis.call('PEXPIRE', k, ttl) end
  redis.call('SADD', KEYS[2], ARGV[i])
end
if #ARGV >= 4 then redis.call('PEXPIRE', KEYS[2], ttl) end
return 1
";

/// `invalidate_tags`: delete every entry of every tag once, with its tag
/// list and its place in other tag sets, and count the entries that were
/// there.
///
/// ARGV: tag-set prefix, entry prefix, tag-list prefix, then the tags.
const INVALIDATE: &str = r"
local seen = {}
local n = 0
for i = 4, #ARGV do
  local tk = ARGV[1] .. ARGV[i]
  for _, e in ipairs(redis.call('SMEMBERS', tk)) do
    if not seen[e] then
      seen[e] = true
      n = n + redis.call('DEL', e)
      local list = ARGV[3] .. string.sub(e, #ARGV[2] + 1)
      for _, t in ipairs(redis.call('SMEMBERS', list)) do
        redis.call('SREM', ARGV[1] .. t, e)
      end
      redis.call('DEL', list)
    end
  end
  redis.call('DEL', tk)
end
return n
";

/// `release_lease`: delete the key only while it is still a lease.
///
/// KEYS: entry.
const RELEASE: &str = r"
if redis.call('GET', KEYS[1]) == 'l' then
  return redis.call('DEL', KEYS[1])
end
return 0
";

struct Scripts {
    set: Script,
    set_tagged: Script,
    invalidate: Script,
    release: Script,
}

/// A cache on Redis or Valkey.
///
/// ```no_run
/// # use std::time::Duration;
/// # use brocanth_cache::{Cache, CacheExt, Namespace};
/// # use brocanth_cache_redis::RedisCache;
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let cache = RedisCache::connect("redis://localhost:6379").await?;
/// let key = Namespace::new("views")?.key("home")?;
/// let page = cache
///     .fetch(&key, Duration::from_secs(60), || async {
///         Ok::<_, std::io::Error>(b"<h1>Home</h1>".to_vec())
///     })
///     .await?;
/// # let _ = page;
/// # Ok(())
/// # }
/// ```
///
/// Cheap to clone: the connection is shared, and it reconnects by itself
/// when the server goes away.
#[derive(Clone)]
pub struct RedisCache {
    conn: ConnectionManager,
    prefix: Arc<str>,
    max_value_bytes: usize,
    scripts: Arc<Scripts>,
}

impl std::fmt::Debug for RedisCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedisCache")
            .field("prefix", &self.prefix)
            .field("max_value_bytes", &self.max_value_bytes)
            .finish_non_exhaustive()
    }
}

fn backend(e: redis::RedisError) -> CacheError {
    CacheError::Backend(Box::new(e))
}

/// A TTL in whole milliseconds, at least one. The shared check refuses a
/// zero TTL and one too long to represent.
fn ttl_ms(ttl: Duration) -> Result<i64> {
    Ok(expires_at(0, ttl)?.max(1))
}

/// Escapes the characters `SCAN MATCH` reads as a pattern.
fn glob_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '\\' | '^') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

impl RedisCache {
    /// A cache at `url`, with no connection open yet.
    ///
    /// The first command connects. This is the constructor for a
    /// synchronous startup stage, where nothing may wait. It must run
    /// inside a Tokio runtime, because the connection's task is spawned
    /// there.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when `url` is not a Redis URL.
    pub fn open(url: &str) -> Result<Self> {
        let client = Client::open(url).map_err(|e| {
            CacheError::Backend(
                format!(
                    "the Redis URL does not parse: {e}\n\
                     help: write `redis://host:6379` or `redis://:password@host:6379/0`"
                )
                .into(),
            )
        })?;
        let config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(TIMEOUT))
            .set_response_timeout(Some(TIMEOUT));
        let conn = ConnectionManager::new_lazy_with_config(client, config).map_err(backend)?;
        Ok(Self {
            conn,
            prefix: Arc::from(DEFAULT_PREFIX),
            max_value_bytes: DEFAULT_MAX_VALUE_BYTES,
            scripts: Arc::new(Scripts {
                set: Script::new(SET),
                set_tagged: Script::new(SET_TAGGED),
                invalidate: Script::new(INVALIDATE),
                release: Script::new(RELEASE),
            }),
        })
    }

    /// A cache at `url`, after one `PING` answered.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when `url` is not a Redis URL, or the server
    /// did not answer.
    pub async fn connect(url: &str) -> Result<Self> {
        let cache = Self::open(url)?;
        cache.ping().await?;
        Ok(cache)
    }

    /// Sends `PING`, and answers whether the server answered.
    ///
    /// # Errors
    /// [`CacheError::Backend`] when it did not.
    pub async fn ping(&self) -> Result<()> {
        let mut conn = self.conn.clone();
        redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .map(drop)
            .map_err(backend)
    }

    /// Writes every key under `prefix` instead of [`DEFAULT_PREFIX`].
    #[must_use]
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = Arc::from(prefix.into());
        self
    }

    /// Changes the cap on one value. Redis takes up to 512 MiB, and a large
    /// value blocks the server while it copies it. See
    /// [`DEFAULT_MAX_VALUE_BYTES`].
    #[must_use]
    pub fn with_max_value_bytes(mut self, bytes: usize) -> Self {
        self.max_value_bytes = bytes;
        self
    }

    /// The prefix every key starts with.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// `{prefix}:e:`, before every entry.
    fn entry_prefix(&self) -> String {
        format!("{}:e:", self.prefix)
    }

    /// `{prefix}:k:`, before every entry's tag list.
    fn list_prefix(&self) -> String {
        format!("{}:k:", self.prefix)
    }

    /// `{prefix}:t:`, before every tag set.
    fn tag_prefix(&self) -> String {
        format!("{}:t:", self.prefix)
    }

    /// The part after the kind prefix: the namespace's length, the
    /// namespace, the key. The length makes a namespace end where it
    /// should, so `clear("a")` does not match `a:b`.
    fn tail(namespace: &Namespace, key: &str) -> String {
        let ns = namespace.as_str();
        format!("{}:{ns}:{key}", ns.len())
    }

    fn entry(&self, key: &Key) -> String {
        format!(
            "{}{}",
            self.entry_prefix(),
            Self::tail(key.namespace(), key.as_str())
        )
    }

    fn list(&self, key: &Key) -> String {
        format!(
            "{}{}",
            self.list_prefix(),
            Self::tail(key.namespace(), key.as_str())
        )
    }

    fn check_size(&self, key: &Key, value: &[u8]) -> Result<()> {
        if value.len() > self.max_value_bytes {
            return Err(CacheError::TooLarge {
                namespace: key.namespace().as_str().to_owned(),
                key: key.as_str().to_owned(),
                bytes: value.len(),
                limit: self.max_value_bytes,
            });
        }
        Ok(())
    }

    fn stored(value: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(value.len() + 1);
        out.push(VALUE);
        out.extend_from_slice(value);
        out
    }

    fn value_of(raw: Option<Vec<u8>>) -> Option<Vec<u8>> {
        match raw {
            Some(mut raw) if raw.first() == Some(&VALUE) => {
                raw.remove(0);
                Some(raw)
            }
            _ => None,
        }
    }

    async fn set_nx(&self, key: &Key, content: &[u8], ttl: Duration) -> Result<bool> {
        let ms = ttl_ms(ttl)?;
        let mut conn = self.conn.clone();
        let answer: Option<String> = redis::cmd("SET")
            .arg(self.entry(key))
            .arg(content)
            .arg("NX")
            .arg("PX")
            .arg(ms)
            .query_async(&mut conn)
            .await
            .map_err(backend)?;
        Ok(answer.is_some())
    }
}

#[async_trait]
impl Cache for RedisCache {
    fn max_value_bytes(&self) -> usize {
        self.max_value_bytes
    }

    /// Redis has nothing to create.
    async fn migrate(&self) -> Result<()> {
        Ok(())
    }

    async fn get(&self, key: &Key) -> Result<Option<Vec<u8>>> {
        let mut conn = self.conn.clone();
        let raw: Option<Vec<u8>> = conn.get(self.entry(key)).await.map_err(backend)?;
        Ok(Self::value_of(raw))
    }

    async fn get_many(&self, keys: &[Key]) -> Result<Vec<Option<Vec<u8>>>> {
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self.conn.clone();
        let names: Vec<String> = keys.iter().map(|k| self.entry(k)).collect();
        let raw: Vec<Option<Vec<u8>>> = redis::cmd("MGET")
            .arg(&names)
            .query_async(&mut conn)
            .await
            .map_err(backend)?;
        Ok(raw.into_iter().map(Self::value_of).collect())
    }

    async fn set(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<()> {
        self.check_size(key, value)?;
        let ms = ttl_ms(ttl)?;
        let mut conn = self.conn.clone();
        self.scripts
            .set
            .key(self.entry(key))
            .key(self.list(key))
            .arg(Self::stored(value))
            .arg(ms)
            .arg(self.tag_prefix())
            .invoke_async::<i64>(&mut conn)
            .await
            .map(drop)
            .map_err(backend)
    }

    async fn set_tagged(&self, key: &Key, value: &[u8], ttl: Duration, tags: &[Tag]) -> Result<()> {
        self.check_size(key, value)?;
        let ms = ttl_ms(ttl)?;
        let mut conn = self.conn.clone();
        let mut call = self.scripts.set_tagged.key(self.entry(key));
        call.key(self.list(key))
            .arg(Self::stored(value))
            .arg(ms)
            .arg(self.tag_prefix());
        for tag in tags {
            call.arg(tag.as_str());
        }
        call.invoke_async::<i64>(&mut conn)
            .await
            .map(drop)
            .map_err(backend)
    }

    async fn invalidate_tags(&self, tags: &[Tag]) -> Result<u64> {
        if tags.is_empty() {
            return Ok(0);
        }
        let mut conn = self.conn.clone();
        let mut call = self.scripts.invalidate.arg(self.tag_prefix());
        call.arg(self.entry_prefix()).arg(self.list_prefix());
        for tag in tags {
            call.arg(tag.as_str());
        }
        let removed: i64 = call.invoke_async(&mut conn).await.map_err(backend)?;
        Ok(removed.max(0) as u64)
    }

    async fn add(&self, key: &Key, value: &[u8], ttl: Duration) -> Result<bool> {
        self.check_size(key, value)?;
        self.set_nx(key, &Self::stored(value), ttl).await
    }

    async fn delete(&self, key: &Key) -> Result<bool> {
        let mut conn = self.conn.clone();
        let removed: i64 = conn.del(self.entry(key)).await.map_err(backend)?;
        Ok(removed > 0)
    }

    async fn clear(&self, namespace: &Namespace) -> Result<u64> {
        let pattern = format!(
            "{}{}",
            glob_escape(&self.entry_prefix()),
            glob_escape(&Self::tail(namespace, "*")).replace("\\*", "*")
        );
        let mut conn = self.conn.clone();
        let mut cursor: u64 = 0;
        let mut removed: u64 = 0;
        loop {
            let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&pattern)
                .arg("COUNT")
                .arg(SCAN_COUNT)
                .query_async(&mut conn)
                .await
                .map_err(backend)?;
            if !keys.is_empty() {
                let gone: i64 = redis::cmd("UNLINK")
                    .arg(&keys)
                    .query_async(&mut conn)
                    .await
                    .map_err(backend)?;
                removed += gone.max(0) as u64;
            }
            if next == 0 {
                return Ok(removed);
            }
            cursor = next;
        }
    }

    async fn lease(&self, key: &Key, ttl: Duration) -> Result<bool> {
        self.set_nx(key, LEASE, ttl).await
    }

    async fn release_lease(&self, key: &Key) -> Result<()> {
        let mut conn = self.conn.clone();
        self.scripts
            .release
            .key(self.entry(key))
            .invoke_async::<i64>(&mut conn)
            .await
            .map(drop)
            .map_err(backend)
    }

    /// Redis expires keys by itself, so there is nothing to purge.
    async fn purge_expired(&self, _limit: u64) -> Result<u64> {
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_ttl_is_refused_and_a_tiny_one_is_one_millisecond() {
        assert!(matches!(
            ttl_ms(Duration::ZERO),
            Err(CacheError::InvalidTtl(_))
        ));
        assert_eq!(ttl_ms(Duration::from_micros(10)).unwrap(), 1);
        assert_eq!(ttl_ms(Duration::from_secs(2)).unwrap(), 2_000);
    }

    #[test]
    fn a_namespace_ends_where_its_length_says() {
        let a = Namespace::new("a").unwrap();
        let ab = Namespace::new("a:b").unwrap();
        assert_eq!(RedisCache::tail(&a, "b:c"), "1:a:b:c");
        assert_eq!(RedisCache::tail(&ab, "c"), "3:a:b:c");
    }

    #[test]
    fn a_pattern_character_in_the_prefix_is_escaped() {
        assert_eq!(glob_escape("app*[1]"), "app\\*\\[1\\]");
    }

    #[test]
    fn only_a_value_reads_as_a_value() {
        assert_eq!(
            RedisCache::value_of(Some(b"vhi".to_vec())),
            Some(b"hi".to_vec())
        );
        assert_eq!(RedisCache::value_of(Some(LEASE.to_vec())), None);
        assert_eq!(RedisCache::value_of(None), None);
    }
}
