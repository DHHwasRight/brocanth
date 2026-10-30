//! Every statement this crate runs, written out.
//!
//! Written out rather than built, for the same reason `brocanth-jobs` writes
//! its two dialects out by hand: these are readable SQL that can be pasted
//! into `psql` when something is wrong, and a statement you can paste is a
//! statement you can explain. The table name is a literal in every one of
//! them, so there is no identifier to quote and nothing to inject.

#![allow(dead_code)]

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Serialises [`SCHEMA`] against every other process running it.
///
/// `CREATE TABLE IF NOT EXISTS` is **not atomic**. The existence check and the
/// creation are separate steps, so two sessions that run it at the same instant
/// both see nothing, both try to create, and the loser fails on
/// `pg_type_typname_nsp_index` with a duplicate key — an error about a system
/// catalogue, which is not a readable way to learn that two of your instances
/// booted together. Rolling deploys do this every time.
///
/// A transaction-level advisory lock fixes it: the first session takes it, the
/// rest wait, and whoever gets in second finds the table already there and
/// creates nothing. The lock is released when the transaction ends, including
/// when it fails, so a crashed migration does not wedge the next boot. The
/// constant is the ASCII of `brocache` read as a 64-bit integer, which makes it
/// identifiable in `pg_locks` and unlikely to collide with anybody else's.
///
/// This is an advisory lock used well, and it is worth contrasting with
/// [`crate::fetch`], which refuses to use one: here it is taken at startup, on
/// one connection, for the length of a few DDL statements. There it would be
/// taken on the request path, by every concurrent reader, and held across a
/// computation — which turns a cache stampede into connection-pool exhaustion.
/// The tool is not the problem; where you hold it is.
pub(crate) const MIGRATION_LOCK: &str = "SELECT pg_advisory_xact_lock(7093854835688302693)";

/// The one table, and the storage parameters that keep it from eating the
/// database.
///
/// # `UNLOGGED`, which is the decision this crate turns on
///
/// An unlogged table writes **no WAL**. The write-ahead log is the thing that
/// makes Postgres durable, and it is also the thing that makes "put your cache
/// in Postgres" sound ridiculous: every `set` would otherwise be written
/// twice, once to the log and once to the heap, and then shipped to every
/// replica and every archive. That write amplification is the usual and
/// correct objection, and `UNLOGGED` deletes it. Writes to an unlogged table
/// are several times cheaper than to a logged one.
///
/// The thing it gives up is exactly the thing a cache does not want. Postgres
/// **truncates every unlogged table on crash recovery**, because without WAL
/// it cannot know the pages are consistent. For a cache, an empty cache after
/// a crash is not damage: it is the correct state. A cache that *survives* a
/// crash is a cache holding entries written by the process that died, which is
/// a cache you now have to reason about invalidating. This is the rare case
/// where the unsafe-sounding option is the safer one.
///
/// **The costs are real and are in [`crate`]'s documentation, not hidden
/// here**: no replica has this table, a crash is a cold cache and therefore a
/// miss storm, and the write pattern makes garbage for autovacuum.
///
/// # Why `fillfactor` is 80 and why `expires_at` has no index
///
/// These two are one decision.
///
/// Postgres has no in-place update: changing a row writes a new version and
/// leaves the old one dead. For a cache — which overwrites the same hot keys
/// over and over — that is the dominant cost, and the mechanism that fixes it
/// is **HOT** (heap-only tuple) updates. A HOT update puts the new version on
/// the same page as the old one and adds **no index entries at all**, so the
/// indexes do not bloat and the dead version's space is reclaimable without
/// touching them.
///
/// HOT needs two things: room on the page, and **no indexed column changed**.
///
/// `fillfactor = 80` is the first: a page is filled to 80% at first write,
/// leaving a fifth of it free for the new versions that overwrites will put
/// there. Twenty per cent of the heap is the price, and it buys the update
/// path.
///
/// No index on `expires_at` is the second, and it is the one that looks
/// strange. An index there would make the sweep
/// ([`PURGE_EXPIRED`]) a range scan instead of a sequential one — but
/// `expires_at` changes on **every single `set`**, so indexing it would mean
/// no overwrite in this cache is ever a HOT update. Every `set` would write a
/// new entry into both indexes, and the index on the most-written column in
/// the table would bloat continuously.
///
/// So: the sweep is a sequential scan, on purpose. It runs every few minutes,
/// not on the request path, and its cost is proportional to the size of a
/// cache you are keeping bounded anyway. The reads it protects are primary-key
/// lookups that never need it. If your cache is big enough that the sweep's
/// sequential scan hurts, the table is big enough that you have outgrown
/// putting it in Postgres — see the crate documentation on when Redis is the
/// answer.
///
/// # The autovacuum settings
///
/// The defaults assume a table that is read far more than it is written. This
/// one is the opposite, so the thresholds are set per-table rather than left
/// for an operator to discover at an awkward moment: vacuum at 2% dead rows
/// instead of 20%, and the same for the TOAST table, where large values leave
/// their own garbage.
///
/// `ALTER TABLE … SET (…)` takes a brief exclusive lock and rewrites nothing,
/// so it is safe to run on every boot, which is what
/// [`migrate`](crate::Cache::migrate) does.
pub(crate) const SCHEMA: &[&str] = &[
    r#"CREATE UNLOGGED TABLE IF NOT EXISTS brocanth_cache (
        namespace  TEXT   NOT NULL,
        k          TEXT   NOT NULL,
        value      BYTEA,
        expires_at BIGINT NOT NULL,
        written_at BIGINT NOT NULL,
        PRIMARY KEY (namespace, k)
    )"#,
    r#"ALTER TABLE brocanth_cache SET (
        fillfactor = 80,
        autovacuum_vacuum_scale_factor = 0.02,
        autovacuum_vacuum_threshold = 1000,
        autovacuum_analyze_scale_factor = 0.05,
        toast.autovacuum_vacuum_scale_factor = 0.02
    )"#,
    // The tag index. A second table and not a `tags TEXT[]` column with a GIN
    // index on `brocanth_cache`, for the reason the `fillfactor` argument
    // above gives: an index on a column that a write changes ends HOT updates
    // for every write. Here only a tagged write touches this table, and a
    // plain `set` keeps its HOT path.
    //
    // Unlogged, like the entries: after a crash both are empty together, and
    // a tag row is never left pointing at an entry that survived.
    r#"CREATE UNLOGGED TABLE IF NOT EXISTS brocanth_cache_tags (
        tag       TEXT NOT NULL,
        namespace TEXT NOT NULL,
        k         TEXT NOT NULL,
        PRIMARY KEY (tag, namespace, k)
    )"#,
    // The other direction: a tagged write replaces the tags of one entry.
    r#"CREATE INDEX IF NOT EXISTS brocanth_cache_tags_entry
        ON brocanth_cache_tags (namespace, k)"#,
];

// ---------------------------------------------------------------------------
// Reads
// ---------------------------------------------------------------------------

/// One entry. A primary-key lookup with the TTL in the predicate, which is
/// where expiry is actually enforced.
///
/// `value` is nullable because a lease is a row with no value; the backend
/// maps that to a miss.
pub(crate) const GET: &str = r#"
    SELECT value
    FROM brocanth_cache
    WHERE namespace = $1 AND k = $2 AND expires_at > $3
"#;

/// Several entries in one round trip.
///
/// `unnest(…) WITH ORDINALITY` turns the two parallel arrays of namespaces and
/// keys into a relation carrying each pair's position, so the backend can put
/// the answers back in the order it was asked. An inner join returns only the
/// hits, which is usually the smaller half, and the caller fills the misses.
pub(crate) const GET_MANY: &str = r#"
    SELECT q.i AS i, c.value AS value
    FROM unnest($1::text[], $2::text[]) WITH ORDINALITY AS q(ns, k, i)
    JOIN brocanth_cache c ON c.namespace = q.ns AND c.k = q.k
    WHERE c.expires_at > $3 AND c.value IS NOT NULL
"#;

// ---------------------------------------------------------------------------
// Writes
// ---------------------------------------------------------------------------

/// Store, replacing whatever is there.
///
/// An upsert rather than a delete-then-insert: one statement, one row version,
/// and a chance at a HOT update. Note that nothing here changes an indexed
/// column, which is what the `fillfactor` argument above is for.
pub(crate) const SET: &str = r#"
    INSERT INTO brocanth_cache (namespace, k, value, expires_at, written_at)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT (namespace, k) DO UPDATE
    SET value = EXCLUDED.value,
        expires_at = EXCLUDED.expires_at,
        written_at = EXCLUDED.written_at
"#;

/// Store, replacing whatever is there, and replace the entry's tags.
///
/// One statement, so the value and its tags are never seen apart. `$6` is the
/// new tag set. The `stale` arm deletes only the tags that are **not** in it
/// and the insert adds the ones that are missing, so the two arms never touch
/// the same row: a delete and an insert of one row in one statement would
/// lose the insert to the conflict check.
///
/// An empty `$6` deletes every tag of the entry.
pub(crate) const SET_TAGGED: &str = r#"
    WITH entry AS (
        INSERT INTO brocanth_cache (namespace, k, value, expires_at, written_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT (namespace, k) DO UPDATE
        SET value = EXCLUDED.value,
            expires_at = EXCLUDED.expires_at,
            written_at = EXCLUDED.written_at
    ),
    stale AS (
        DELETE FROM brocanth_cache_tags
        WHERE namespace = $1 AND k = $2 AND NOT (tag = ANY($6::text[]))
    )
    INSERT INTO brocanth_cache_tags (tag, namespace, k)
    SELECT DISTINCT t, $1::text, $2::text FROM unnest($6::text[]) AS t
    ON CONFLICT DO NOTHING
"#;

/// Remove every entry carrying any of the tags in `$1`, and those tag rows.
///
/// The tag rows go first, in a CTE, and hand their entries to the outer
/// delete. A primary-key range on `tag` finds them. The entries' other tag
/// rows stay behind until [`PURGE_ORPHAN_TAGS`] finds them; they point at
/// nothing, so they cost space and never a wrong answer.
pub(crate) const INVALIDATE_TAGS: &str = r#"
    WITH tagged AS (
        DELETE FROM brocanth_cache_tags
        WHERE tag = ANY($1::text[])
        RETURNING namespace, k
    )
    DELETE FROM brocanth_cache c
    USING (SELECT DISTINCT namespace, k FROM tagged) t
    WHERE c.namespace = t.namespace AND c.k = t.k
"#;

/// Store only if nothing live is there. Returns a row when it stored.
///
/// The `WHERE` on the conflict arm is the whole mechanism: `ON CONFLICT DO
/// NOTHING` alone would be blocked forever by an expired row that nobody has
/// swept, so the conflict arm takes the row over **only when the existing one
/// has already expired**. `$5` is the current instant, which is both the
/// `written_at` being stored and the instant the existing row is judged
/// against.
///
/// `ON CONFLICT DO UPDATE` locks the conflicting row, so two callers racing on
/// one key serialise inside the statement and exactly one gets a row back.
/// There is no check-then-write to lose a race in.
pub(crate) const ADD: &str = r#"
    INSERT INTO brocanth_cache (namespace, k, value, expires_at, written_at)
    VALUES ($1, $2, $3, $4, $5)
    ON CONFLICT (namespace, k) DO UPDATE
    SET value = EXCLUDED.value,
        expires_at = EXCLUDED.expires_at,
        written_at = EXCLUDED.written_at
    WHERE brocanth_cache.expires_at <= $5
    RETURNING 1
"#;

/// Take the claim on a key: [`ADD`] with no value.
///
/// A row whose `value` is NULL is a lease. [`GET`] returns NULL for it, which
/// the backend reads as a miss, so a lease never looks like a value to
/// anybody.
pub(crate) const LEASE: &str = r#"
    INSERT INTO brocanth_cache (namespace, k, value, expires_at, written_at)
    VALUES ($1, $2, NULL, $3, $4)
    ON CONFLICT (namespace, k) DO UPDATE
    SET value = NULL,
        expires_at = EXCLUDED.expires_at,
        written_at = EXCLUDED.written_at
    WHERE brocanth_cache.expires_at <= $4
    RETURNING 1
"#;

/// Give the claim back, without touching a value somebody else stored.
///
/// `value IS NULL` is the guard. If this lease expired and another caller took
/// it over and stored a real value, this statement removes nothing — which is
/// the difference between releasing a lease and deleting an entry.
pub(crate) const RELEASE_LEASE: &str = r#"
    DELETE FROM brocanth_cache
    WHERE namespace = $1 AND k = $2 AND value IS NULL
"#;

/// Remove one entry.
pub(crate) const DELETE: &str = r#"
    DELETE FROM brocanth_cache WHERE namespace = $1 AND k = $2
"#;

/// Remove a whole namespace.
///
/// Equality on the leading column of the primary key, so an index range rather
/// than the prefix match a single concatenated key column would have forced.
pub(crate) const CLEAR: &str = r#"
    DELETE FROM brocanth_cache WHERE namespace = $1
"#;

/// Remove up to `$2` expired entries.
///
/// Postgres has no `DELETE … LIMIT`, so the bound goes on a subquery selecting
/// `ctid` — the physical row address, which needs no index and is the cheapest
/// thing to delete by. The scan is sequential; see [`SCHEMA`] for why that is
/// a choice rather than an oversight.
pub(crate) const PURGE_EXPIRED: &str = r#"
    DELETE FROM brocanth_cache
    WHERE ctid IN (
        SELECT ctid FROM brocanth_cache WHERE expires_at <= $1 LIMIT $2
    )
"#;

/// Remove up to `$2` tag rows whose entry is gone or expired.
///
/// The sweep's second half. Run after [`PURGE_EXPIRED`], so the entries it
/// just removed leave their tags here. The anti-join is a primary-key lookup
/// on `brocanth_cache` per tag row.
pub(crate) const PURGE_ORPHAN_TAGS: &str = r#"
    DELETE FROM brocanth_cache_tags
    WHERE ctid IN (
        SELECT t.ctid FROM brocanth_cache_tags t
        WHERE NOT EXISTS (
            SELECT 1 FROM brocanth_cache c
            WHERE c.namespace = t.namespace AND c.k = t.k AND c.expires_at > $1
        )
        LIMIT $2
    )
"#;

// ---------------------------------------------------------------------------
// Looking at it
// ---------------------------------------------------------------------------

/// Whether the table really is unlogged.
///
/// Worth asserting in a deployment check. A table created by an older
/// migration, restored from a `pg_dump`, or made by hand will be a perfectly
/// ordinary logged table — everything works, and every write goes through the
/// WAL and out to the replicas, which is the cost this design exists to avoid.
/// Nothing in the running system notices, so something has to ask.
pub(crate) const IS_UNLOGGED: &str = r#"
    SELECT c.relpersistence = 'u'
    FROM pg_class c
    WHERE c.relname = 'brocanth_cache'
      AND c.relkind = 'r'
      AND pg_catalog.pg_table_is_visible(c.oid)
"#;

/// The numbers to put on a dashboard.
///
/// `dead_tuples` and `last_vacuum` come from `pg_stat_user_tables` and are the
/// pair that matters: this table makes garbage faster than any other table in
/// the database, and dead tuples climbing while `last_vacuum` stands still is
/// autovacuum losing the race. That is the signal that the cache is costing
/// the rest of the database, and it arrives long before anything looks broken.
pub(crate) const STATS: &str = r#"
    SELECT
        (SELECT count(*) FROM brocanth_cache)::BIGINT AS entries,
        (SELECT count(*) FROM brocanth_cache WHERE expires_at <= $1)::BIGINT AS expired,
        pg_total_relation_size('brocanth_cache')::BIGINT AS bytes,
        COALESCE((SELECT n_dead_tup FROM pg_stat_user_tables
                  WHERE relid = 'brocanth_cache'::regclass), 0)::BIGINT AS dead_tuples,
        (SELECT (extract(epoch from greatest(last_vacuum, last_autovacuum)) * 1000)::BIGINT
         FROM pg_stat_user_tables WHERE relid = 'brocanth_cache'::regclass) AS last_vacuum
"#;
