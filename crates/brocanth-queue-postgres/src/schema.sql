-- The queue's schema.
--
-- Four tables with four different access patterns, and therefore four
-- different sets of storage parameters. Putting them in one table and
-- separating them with a `state` column is the mistake this schema exists to
-- avoid: one table cannot be both a hot claim target and a cold archive.

-- ---------------------------------------------------------------------------
-- 1. The bulk queue. Hot. Rows are deleted when they succeed.
-- ---------------------------------------------------------------------------
--
-- `available_at` is the whole mechanism. It means "not claimable before
-- this", and a claim sets it to `now() + lease`. So a lease is just a row
-- that is not claimable yet, and lease expiry needs no sweeper: a worker
-- that dies leaves a row that becomes claimable again on its own. One
-- column, one index, no background job for the case that matters most.

-- `claimed_by` and `claimed_at` are the admin screens and nothing else. No
-- statement reads them, so they are never in a WHERE clause and they carry
-- no index: a "what is running right now" screen reads the right edge of
-- the claim index, where the leased rows are, because a lease is a row with
-- `available_at` in the future. They are written by the claim, which is
-- already an UPDATE of that row, so they cost no extra write.
--
-- Without them a worker roster has to be its own table with its own
-- heartbeat, which is a second kind of state that can disagree with the
-- first. `ClaimRequest::worker` is already on the wire; this is where it
-- lands.
--
-- fillfactor 70: a claim updates `available_at`, which is indexed, so the
-- update can never be HOT. It still pays to keep the new row version on the
-- same page: the heap write stays local and the page splits less. Fillfactor
-- does not avoid the index write, and no setting can, because the index is
-- what makes the row invisible.
--
-- autovacuum: the scale factor is set to 0 and the threshold to a flat row
-- count on purpose. The default `0.2` means a ten million row queue waits for
-- two million dead tuples before it vacuums, by which time the claim index is
-- bloated and the claim is slow. A queue wants a vacuum on a fixed small
-- number of dead rows, and wants it to run at full speed.
CREATE TABLE IF NOT EXISTS queue_bulk (
    id            bigserial PRIMARY KEY,
    kind          text        NOT NULL,
    queue         text        NOT NULL,
    payload       bytea       NOT NULL,
    available_at  timestamptz NOT NULL DEFAULT now(),
    attempts      integer     NOT NULL DEFAULT 0,
    max_attempts  integer     NOT NULL,
    lease_token   uuid,
    claimed_by    text,
    claimed_at    timestamptz,
    unique_key    text,
    uniqueness    text        NOT NULL DEFAULT 'none',
    enqueued_at   timestamptz NOT NULL DEFAULT now(),
    last_error    text
) WITH (
    fillfactor = 70,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 1000,
    autovacuum_vacuum_cost_delay = 0,
    autovacuum_analyze_scale_factor = 0,
    autovacuum_analyze_threshold = 1000
);

-- The claim index. Leading on `queue` because a worker claims from one
-- queue, then on `available_at` because the claim scans from the earliest.
-- A row scheduled for next week sits at the far right of this index and is
-- never touched by a claim, which is why a short delay does not need the
-- scheduled table.
CREATE INDEX IF NOT EXISTS queue_bulk_claim
    ON queue_bulk (queue, available_at)
    WITH (fillfactor = 70);

-- Deduplication for `until_success`: the key is held by the row's own
-- existence, so a partial unique index is the whole implementation. A
-- `window` key does not live here, because it has to outlive the row.
CREATE UNIQUE INDEX IF NOT EXISTS queue_bulk_unique_key
    ON queue_bulk (unique_key)
    WHERE unique_key IS NOT NULL;

-- ---------------------------------------------------------------------------
-- 2. Delayed work. Cold, range scanned over a moving boundary.
-- ---------------------------------------------------------------------------
--
-- A promoter moves rows here into `queue_bulk` as they come due. Three
-- reasons it is a separate table:
--
-- * The access patterns disagree. The claim reads the left edge of a hot
--   index; the promoter reads a range over a boundary that moves. Mixing
--   them makes the claim index hold millions of rows the claim never reads,
--   which makes it deeper and colder.
-- * The vacuum settings disagree. The claim target churns and wants an
--   aggressive autovacuum; the delayed set is nearly static and does not.
-- * It is what makes `max_delay` unbounded on a backend that has a ceiling.
--   SQS caps a delay at fifteen minutes and JetStream has no per-message
--   schedule at all. The promoter holds the row until it is nearly due and
--   then pushes it, so a three day delay works on a backend that cannot
--   express one.
--
-- The index's left edge is deleted constantly, which bloats a btree. There
-- is no storage parameter that prevents that; the answer is an aggressive
-- autovacuum and a periodic REINDEX CONCURRENTLY, and this comment exists so
-- nobody looks for a cleverer one.
CREATE TABLE IF NOT EXISTS queue_scheduled (
    id            bigserial PRIMARY KEY,
    kind          text        NOT NULL,
    queue         text        NOT NULL,
    payload       bytea       NOT NULL,
    run_at        timestamptz NOT NULL,
    max_attempts  integer     NOT NULL,
    unique_key    text,
    uniqueness    text        NOT NULL DEFAULT 'none',
    enqueued_at   timestamptz NOT NULL DEFAULT now()
) WITH (
    fillfactor = 90,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 500,
    autovacuum_vacuum_cost_delay = 0
);

CREATE INDEX IF NOT EXISTS queue_scheduled_due
    ON queue_scheduled (run_at);

-- ---------------------------------------------------------------------------
-- 3. The transactional queue. Small, in the application's own database.
-- ---------------------------------------------------------------------------
--
-- The same shape as `queue_bulk` and deliberately not the same table. This
-- one is written inside the caller's transaction and is bounded by user
-- actions, so it is small, and it must never be migrated anywhere. Keeping
-- it separate is what makes "this lane never moves" a fact about the schema
-- rather than a promise in a document.
CREATE TABLE IF NOT EXISTS queue_tx (
    id            bigserial PRIMARY KEY,
    kind          text        NOT NULL,
    queue         text        NOT NULL,
    payload       bytea       NOT NULL,
    available_at  timestamptz NOT NULL DEFAULT now(),
    attempts      integer     NOT NULL DEFAULT 0,
    max_attempts  integer     NOT NULL,
    lease_token   uuid,
    claimed_by    text,
    claimed_at    timestamptz,
    unique_key    text,
    uniqueness    text        NOT NULL DEFAULT 'none',
    enqueued_at   timestamptz NOT NULL DEFAULT now(),
    last_error    text
) WITH (
    fillfactor = 70,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 200,
    autovacuum_vacuum_cost_delay = 0
);

CREATE INDEX IF NOT EXISTS queue_tx_claim
    ON queue_tx (queue, available_at)
    WITH (fillfactor = 70);

CREATE UNIQUE INDEX IF NOT EXISTS queue_tx_unique_key
    ON queue_tx (unique_key)
    WHERE unique_key IS NOT NULL;

-- ---------------------------------------------------------------------------
-- 4. The outbox. Written in the caller's transaction, drained by a relay.
-- ---------------------------------------------------------------------------
--
-- `available_at` carries the relay's own lease, the same trick as the queue:
-- a relay that dies does not strand rows.
CREATE TABLE IF NOT EXISTS queue_outbox (
    id            bigserial PRIMARY KEY,
    kind          text        NOT NULL,
    queue         text        NOT NULL,
    payload       bytea       NOT NULL,
    run_at        timestamptz,
    max_attempts  integer     NOT NULL,
    unique_key    text,
    uniqueness    text        NOT NULL DEFAULT 'none',
    available_at  timestamptz NOT NULL DEFAULT now(),
    attempts      integer     NOT NULL DEFAULT 0,
    last_error    text,
    enqueued_at   timestamptz NOT NULL DEFAULT now()
) WITH (
    fillfactor = 70,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 1000,
    autovacuum_vacuum_cost_delay = 0
);

CREATE INDEX IF NOT EXISTS queue_outbox_ready
    ON queue_outbox (available_at)
    WITH (fillfactor = 70);

-- The outbox's own dead letter. A row lands here when the backend rejects
-- it for good, or when it fails `RelayConfig::max_attempts` times while the
-- rows beside it go through. Without it, one row the backend will never
-- accept holds back its batch and then the table.
--
-- It is not `queue_dead`. A row here never reached the backend, so a
-- requeue puts it back in `queue_outbox` and the relay delivers it again.
-- It keeps every column the relay needs to rebuild the job: `run_at` and
-- `uniqueness` too, which `queue_dead` does not have.
--
-- `outbox_id` is the id the row had in `queue_outbox`, which is what a
-- relay's log line names. The primary key is the dead letter's own.
--
-- Small and read by a person, so no storage parameters. A new table needs
-- no line in section 8: `CREATE TABLE IF NOT EXISTS` reaches an existing
-- database as well as a new one.
CREATE TABLE IF NOT EXISTS queue_outbox_dead (
    id            bigserial PRIMARY KEY,
    outbox_id     bigint      NOT NULL,
    kind          text        NOT NULL,
    queue         text        NOT NULL,
    payload       bytea       NOT NULL,
    run_at        timestamptz,
    max_attempts  integer     NOT NULL,
    unique_key    text,
    uniqueness    text        NOT NULL DEFAULT 'none',
    attempts      integer     NOT NULL,
    failure       text        NOT NULL,
    enqueued_at   timestamptz NOT NULL,
    died_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS queue_outbox_dead_recent ON queue_outbox_dead (died_at DESC);

-- ---------------------------------------------------------------------------
-- 5. Deduplication windows that outlive their row.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS queue_dedup (
    key        text PRIMARY KEY,
    expires_at timestamptz NOT NULL
) WITH (
    fillfactor = 80,
    autovacuum_vacuum_scale_factor = 0,
    autovacuum_vacuum_threshold = 1000
);

CREATE INDEX IF NOT EXISTS queue_dedup_expiry ON queue_dedup (expires_at);

-- ---------------------------------------------------------------------------
-- 6. The dead letter. Small, queried by a person, never by the claim path.
-- ---------------------------------------------------------------------------
--
-- `max_attempts` and `unique_key` are here so a row can be put back. A dead
-- letter you cannot requeue is a log line with extra steps, and the two
-- columns are the whole difference. `max_attempts` is nullable because a row
-- written before this column existed has no answer; a requeue of one of
-- those uses the kind's registered default.
--
-- `job_id` is the id the row had while it was live, which is what ties it to
-- a log line and to `queue_archive`. The primary key here is the dead
-- letter's own, because that is what a requeue or a discard names.
CREATE TABLE IF NOT EXISTS queue_dead (
    id           bigserial PRIMARY KEY,
    job_id       bigint,
    kind         text        NOT NULL,
    queue        text        NOT NULL,
    payload      bytea       NOT NULL,
    attempts     integer     NOT NULL,
    max_attempts integer,
    unique_key   text,
    failure      text        NOT NULL,
    enqueued_at  timestamptz NOT NULL,
    died_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS queue_dead_recent ON queue_dead (died_at DESC);

-- ---------------------------------------------------------------------------
-- 7. The archive, partitioned by day.
-- ---------------------------------------------------------------------------
--
-- This is where partitioning belongs, and it is not where it is usually put.
-- Partitioning the live queue so that cleanup is `DROP PARTITION` does not
-- work: the thing being cleaned up is finished jobs, a partition cannot be
-- dropped while one row in it is still retrying, and a retry can be days
-- after the enqueue. So the live table deletes on success and the archive,
-- which is append only and strictly ordered by `finished_at`, is the part
-- that gets partitions.
--
-- A day's partition is dropped whole. No row is ever deleted from the
-- archive, so autovacuum never has to visit it.
--
-- The archive is optional. A deployment that keeps nothing simply never
-- creates it, and `Settle::Done` is a plain DELETE.
CREATE TABLE IF NOT EXISTS queue_archive (
    id           bigint      NOT NULL,
    kind         text        NOT NULL,
    queue        text        NOT NULL,
    attempts     integer     NOT NULL,
    enqueued_at  timestamptz NOT NULL,
    finished_at  timestamptz NOT NULL
) PARTITION BY RANGE (finished_at);

-- ---------------------------------------------------------------------------
-- 8. Upgrades.
-- ---------------------------------------------------------------------------
--
-- This file is applied with `CREATE TABLE IF NOT EXISTS`, which does nothing
-- at all to a table that is already there. So a column added above reaches a
-- new database and no existing one. Every such column is repeated here as an
-- `ADD COLUMN IF NOT EXISTS`, which is idempotent and runs in milliseconds:
-- none of them has a default, so Postgres only rewrites the catalogue.
--
-- The definitions above stay the truth. These restate them for a database
-- that was created before they were written.
ALTER TABLE queue_bulk ADD COLUMN IF NOT EXISTS claimed_by text;
ALTER TABLE queue_bulk ADD COLUMN IF NOT EXISTS claimed_at timestamptz;
ALTER TABLE queue_tx   ADD COLUMN IF NOT EXISTS claimed_by text;
ALTER TABLE queue_tx   ADD COLUMN IF NOT EXISTS claimed_at timestamptz;
ALTER TABLE queue_dead ADD COLUMN IF NOT EXISTS job_id bigint;
ALTER TABLE queue_dead ADD COLUMN IF NOT EXISTS max_attempts integer;
ALTER TABLE queue_dead ADD COLUMN IF NOT EXISTS unique_key text;
