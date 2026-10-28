//! The screens, in SQL.
//!
//! Everything here reads columns the claim path already maintains. There is
//! no `state` column, no counter table and no second source of truth: a
//! state is a predicate over `available_at`, `lease_token` and `attempts`,
//! which is why two processes serving these screens always agree.
//!
//! # Indexes
//!
//! None are added for this module, on purpose. Every list here is bounded
//! by a `LIMIT` and read by a person pressing a button; the claim runs
//! thousands of times a second. A partial index on `lease_token IS NOT
//! NULL` would make the "running" screen an index scan and would add an
//! index write to every claim and every settle, which is the wrong side of
//! that trade by several orders of magnitude.
//!
//! The one list that does use an index is `ready`: those rows sit at the
//! left edge of `queue_bulk_claim (queue, available_at)`, where the claim
//! itself reads.
//!
//! # Two tables behind one screen
//!
//! A short delay stays in the live table with `available_at` in the future.
//! A long one is held in `queue_scheduled` until the promoter brings it
//! over. Both are "scheduled" to a person, so [`View::Scheduled`] reads
//! both and tags the ids: a row from `queue_scheduled` has an `s` in front
//! of its id, and [`Inspect::run_now`] and [`Inspect::discard`] send it to
//! the right table. Nothing outside this file sees the difference.

use std::time::{Duration, SystemTime};

use brocanth_queue::{
    BackendError, Bucket, DeadId, DeadRow, Finished, Inspect, JobId, JobRow, Page, QueueDepth,
    Stats, View,
};
use chrono::{DateTime, Utc};
use sqlx::{AssertSqlSafe, Row};

use crate::{err, PgQueue, Table};

/// The attempt budget a requeue gives a dead row that does not record one.
///
/// Only reachable for a row written before `queue_dead.max_attempts`
/// existed. A row written since carries its own answer and this is not
/// consulted.
const REQUEUE_DEFAULT_ATTEMPTS: i32 = 25;

/// The `WHERE` fragment that recognises one state in the live table.
///
/// These four are exhaustive over the rows a worker can act on, and they do
/// not overlap. They leave out exactly one case: a row with its attempts
/// spent that the sweep has not moved to the dead letter yet, which exists
/// for at most one sweep interval and belongs to no screen.
fn predicate(view: View) -> &'static str {
    match view {
        View::Ready => "available_at <= now() AND attempts < max_attempts",
        View::Running => "available_at > now() AND lease_token IS NOT NULL",
        View::Retrying => "available_at > now() AND lease_token IS NULL AND attempts > 0",
        View::Scheduled => "available_at > now() AND lease_token IS NULL AND attempts = 0",
    }
}

/// Which table an id from these screens names.
///
/// The two tables number their rows independently, so an id is only
/// meaningful with the table it came from. Tagging it is what lets one
/// screen list both.
enum Ref {
    /// A row in `queue_bulk` or `queue_tx`.
    Live(i64),
    /// A row in `queue_scheduled`.
    Scheduled(i64),
}

fn parse_ref(id: &JobId) -> Option<Ref> {
    match id.as_str().strip_prefix('s') {
        Some(rest) => rest.parse().ok().map(Ref::Scheduled),
        None => id.as_str().parse().ok().map(Ref::Live),
    }
}

fn live_id(n: i64) -> JobId {
    JobId(n.to_string())
}

fn scheduled_id(n: i64) -> JobId {
    JobId(format!("s{n}"))
}

/// Seconds, as a `Duration`, from a nullable `extract(epoch …)`.
///
/// Zero is a real answer and `NULL` is not: a queue with one job that
/// became ready this instant has an oldest-ready age of zero, and a queue
/// with nothing ready has none. Collapsing the two would make an idle queue
/// and a keeping-up queue look the same on the dashboard.
fn age(secs: Option<f64>) -> Option<Duration> {
    secs.map(|s| Duration::from_secs_f64(s.max(0.0)))
}

fn when(t: DateTime<Utc>) -> SystemTime {
    SystemTime::from(t)
}

/// Read a live-table row into the shape the screens take.
fn live_row(r: &sqlx::postgres::PgRow) -> JobRow {
    JobRow {
        id: live_id(r.get("id")),
        kind: r.get("kind"),
        queue: r.get("queue"),
        payload: r.get("payload"),
        attempt: r.get::<i32, _>("attempts") as u32,
        max_attempts: r.get::<i32, _>("max_attempts") as u32,
        enqueued_at: when(r.get("enqueued_at")),
        available_at: when(r.get("available_at")),
        claimed_by: r.get("claimed_by"),
        claimed_at: r.get::<Option<DateTime<Utc>>, _>("claimed_at").map(when),
        last_error: r.get("last_error"),
    }
}

fn dead_row(r: &sqlx::postgres::PgRow) -> DeadRow {
    DeadRow {
        id: DeadId(r.get::<i64, _>("id").to_string()),
        job_id: r.get::<Option<i64>, _>("job_id").map(live_id),
        kind: r.get("kind"),
        queue: r.get("queue"),
        payload: r.get("payload"),
        attempts: r.get::<i32, _>("attempts") as u32,
        max_attempts: r.get::<Option<i32>, _>("max_attempts").map(|n| n as u32),
        unique_key: r.get("unique_key"),
        failure: r.get("failure"),
        enqueued_at: when(r.get("enqueued_at")),
        died_at: when(r.get("died_at")),
    }
}

impl PgQueue {
    /// Whether `queue_scheduled` is this queue's to read.
    ///
    /// The promoter only moves rows into `queue_bulk`, so the delayed table
    /// belongs to the bulk lane. A transactional queue that counted it
    /// would report another lane's backlog as its own.
    fn owns_scheduled(&self) -> bool {
        self.config.table == Table::Bulk
    }

    async fn count(&self, sql: &str) -> Result<u64, BackendError> {
        let row = sqlx::query(AssertSqlSafe(sql.to_string()))
            .fetch_one(&self.pool)
            .await
            .map_err(err)?;
        Ok(row.get::<i64, _>(0).max(0) as u64)
    }
}

impl Inspect for PgQueue {
    fn has_history(&self) -> bool {
        self.config.archive
    }

    async fn stats(&self) -> Result<Stats, BackendError> {
        let table = self.config.table.name();
        let (ready, running, retrying, scheduled) = (
            predicate(View::Ready),
            predicate(View::Running),
            predicate(View::Retrying),
            predicate(View::Scheduled),
        );
        // One pass over the live table. `FILTER` is what makes four counts
        // and a minimum one sequential scan instead of five.
        //
        // `extract(epoch ...)` returns `numeric`, not a float, and has done
        // since Postgres 14. The cast is what keeps the decode from failing
        // at run time on the one column that is read for a dashboard.
        let sql = format!(
            "SELECT count(*) FILTER (WHERE {ready})     AS ready, \
                    count(*) FILTER (WHERE {running})   AS running, \
                    count(*) FILTER (WHERE {retrying})  AS retrying, \
                    count(*) FILTER (WHERE {scheduled}) AS scheduled, \
                    extract(epoch FROM now() - min(available_at) \
                            FILTER (WHERE {ready}))::float8 AS oldest \
               FROM {table}"
        );
        let r = sqlx::query(AssertSqlSafe(sql))
            .fetch_one(&self.pool)
            .await
            .map_err(err)?;

        let delayed = if self.owns_scheduled() {
            self.count("SELECT count(*) FROM queue_scheduled").await?
        } else {
            0
        };
        // The dead letter is one table for both lanes, because a dead job
        // is read by a person and the lane it came from is not what they
        // are looking for. A deployment that wants them apart splits on the
        // `queue` name, which is in every row.
        let dead = self.count("SELECT count(*) FROM queue_dead").await?;

        Ok(Stats {
            ready: r.get::<i64, _>("ready").max(0) as u64,
            running: r.get::<i64, _>("running").max(0) as u64,
            retrying: r.get::<i64, _>("retrying").max(0) as u64,
            scheduled: r.get::<i64, _>("scheduled").max(0) as u64 + delayed,
            dead,
            oldest_ready: age(r.get("oldest")),
        })
    }

    async fn depths(&self) -> Result<Vec<QueueDepth>, BackendError> {
        let table = self.config.table.name();
        let (ready, running, retrying, scheduled) = (
            predicate(View::Ready),
            predicate(View::Running),
            predicate(View::Retrying),
            predicate(View::Scheduled),
        );
        let sql = format!(
            "SELECT queue, \
                    count(*) FILTER (WHERE {ready})     AS ready, \
                    count(*) FILTER (WHERE {running})   AS running, \
                    count(*) FILTER (WHERE {retrying})  AS retrying, \
                    count(*) FILTER (WHERE {scheduled}) AS scheduled, \
                    extract(epoch FROM now() - min(available_at) \
                            FILTER (WHERE {ready}))::float8 AS oldest \
               FROM {table} \
              GROUP BY queue \
              ORDER BY queue"
        );
        let mut out: Vec<QueueDepth> = sqlx::query(AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await
            .map_err(err)?
            .iter()
            .map(|r| QueueDepth {
                queue: r.get("queue"),
                ready: r.get::<i64, _>("ready").max(0) as u64,
                running: r.get::<i64, _>("running").max(0) as u64,
                retrying: r.get::<i64, _>("retrying").max(0) as u64,
                scheduled: r.get::<i64, _>("scheduled").max(0) as u64,
                oldest_ready: age(r.get("oldest")),
            })
            .collect();

        // A queue whose every job is delayed has no row in the live table
        // yet, so it is missing from the grouping above. It still has a
        // backlog and still belongs on the screen.
        if self.owns_scheduled() {
            let delayed = sqlx::query(
                "SELECT queue, count(*) AS n FROM queue_scheduled GROUP BY queue ORDER BY queue",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(err)?;
            for r in &delayed {
                let name: String = r.get("queue");
                let n = r.get::<i64, _>("n").max(0) as u64;
                match out.iter_mut().find(|q| q.queue == name) {
                    Some(q) => q.scheduled += n,
                    None => out.push(QueueDepth {
                        queue: name,
                        ready: 0,
                        running: 0,
                        retrying: 0,
                        scheduled: n,
                        oldest_ready: None,
                    }),
                }
            }
            out.sort_by(|a, b| a.queue.cmp(&b.queue));
        }
        Ok(out)
    }

    async fn list(&self, view: View, page: Page) -> Result<Vec<JobRow>, BackendError> {
        let table = self.config.table.name();
        let p = predicate(view);
        // The sort is what makes a first page worth reading. Ready and
        // retrying lead with the oldest, which is the one that is late;
        // running leads with the longest held, which is the one that is
        // stuck.
        let order = match view {
            View::Ready | View::Retrying | View::Scheduled => "available_at ASC",
            View::Running => "claimed_at ASC NULLS LAST",
        };
        let limit = page.limit.min(Page::MAX_LIMIT) as i64;
        let sql = format!(
            "SELECT id, kind, queue, payload, attempts, max_attempts, enqueued_at, \
                    available_at, claimed_by, claimed_at, last_error \
               FROM {table} \
              WHERE {p} \
              ORDER BY {order} \
              LIMIT $1 OFFSET $2"
        );
        let mut rows: Vec<JobRow> = sqlx::query(AssertSqlSafe(sql))
            .bind(limit)
            .bind(page.offset as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(err)?
            .iter()
            .map(live_row)
            .collect();

        // The delayed table is the rest of the scheduled screen. It is read
        // second and appended rather than merged with a SQL `UNION`,
        // because the two tables have different columns and a union would
        // need every missing one spelled as a literal. The order is still
        // right: a row in `queue_scheduled` is further out than one the
        // promoter has already brought over.
        if view == View::Scheduled && self.owns_scheduled() {
            let want = limit - rows.len() as i64;
            if want > 0 {
                let skip = (page.offset as i64 - rows.len() as i64).max(0);
                let delayed = sqlx::query(
                    "SELECT id, kind, queue, payload, max_attempts, enqueued_at, run_at \
                       FROM queue_scheduled \
                      ORDER BY run_at ASC \
                      LIMIT $1 OFFSET $2",
                )
                .bind(want)
                .bind(skip)
                .fetch_all(&self.pool)
                .await
                .map_err(err)?;
                rows.extend(delayed.iter().map(|r| JobRow {
                    id: scheduled_id(r.get("id")),
                    kind: r.get("kind"),
                    queue: r.get("queue"),
                    payload: r.get("payload"),
                    attempt: 0,
                    max_attempts: r.get::<i32, _>("max_attempts") as u32,
                    enqueued_at: when(r.get("enqueued_at")),
                    available_at: when(r.get("run_at")),
                    claimed_by: None,
                    claimed_at: None,
                    last_error: None,
                }));
            }
        }
        Ok(rows)
    }

    async fn job(&self, id: &JobId) -> Result<Option<JobRow>, BackendError> {
        let table = self.config.table.name();
        match parse_ref(id) {
            None => Ok(None),
            Some(Ref::Live(n)) => {
                let sql = format!(
                    "SELECT id, kind, queue, payload, attempts, max_attempts, enqueued_at, \
                            available_at, claimed_by, claimed_at, last_error \
                       FROM {table} WHERE id = $1"
                );
                Ok(sqlx::query(AssertSqlSafe(sql))
                    .bind(n)
                    .fetch_optional(&self.pool)
                    .await
                    .map_err(err)?
                    .as_ref()
                    .map(live_row))
            }
            Some(Ref::Scheduled(n)) => Ok(sqlx::query(
                "SELECT id, kind, queue, payload, max_attempts, enqueued_at, run_at \
                   FROM queue_scheduled WHERE id = $1",
            )
            .bind(n)
            .fetch_optional(&self.pool)
            .await
            .map_err(err)?
            .map(|r| JobRow {
                id: scheduled_id(r.get("id")),
                kind: r.get("kind"),
                queue: r.get("queue"),
                payload: r.get("payload"),
                attempt: 0,
                max_attempts: r.get::<i32, _>("max_attempts") as u32,
                enqueued_at: when(r.get("enqueued_at")),
                available_at: when(r.get("run_at")),
                claimed_by: None,
                claimed_at: None,
                last_error: None,
            })),
        }
    }

    async fn dead(&self, page: Page) -> Result<Vec<DeadRow>, BackendError> {
        let limit = page.limit.min(Page::MAX_LIMIT) as i64;
        Ok(sqlx::query(
            "SELECT id, job_id, kind, queue, payload, attempts, max_attempts, unique_key, \
                    failure, enqueued_at, died_at \
               FROM queue_dead \
              ORDER BY died_at DESC \
              LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(page.offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(err)?
        .iter()
        .map(dead_row)
        .collect())
    }

    async fn dead_one(&self, id: &DeadId) -> Result<Option<DeadRow>, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        Ok(sqlx::query(
            "SELECT id, job_id, kind, queue, payload, attempts, max_attempts, unique_key, \
                    failure, enqueued_at, died_at \
               FROM queue_dead WHERE id = $1",
        )
        .bind(n)
        .fetch_optional(&self.pool)
        .await
        .map_err(err)?
        .as_ref()
        .map(dead_row))
    }

    async fn run_now(&self, id: &JobId) -> Result<bool, BackendError> {
        let table = self.config.table.name();
        match parse_ref(id) {
            None => Ok(false),
            // `lease_token IS NULL OR available_at <= now()` is the whole
            // safety condition. A held lease is refused, because clearing
            // one is how two workers come to run the same job. An *expired*
            // lease still carries the dead worker's token — nothing clears
            // it, since expiry is `available_at` passing and not an event
            // anyone runs — so the second half of the condition is what
            // lets an orphaned job be pushed along.
            Some(Ref::Live(n)) => {
                let sql = format!(
                    "UPDATE {table} \
                        SET available_at = now(), lease_token = NULL, \
                            claimed_by = NULL, claimed_at = NULL \
                      WHERE id = $1 \
                        AND (lease_token IS NULL OR available_at <= now())"
                );
                Ok(sqlx::query(AssertSqlSafe(sql))
                    .bind(n)
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
                    > 0)
            }
            // A delayed row is promoted rather than edited in place: its
            // table has no `available_at` and no worker reads it. This is
            // the promoter's statement for one row.
            Some(Ref::Scheduled(n)) => {
                let sql = format!(
                    "WITH due AS ( \
                         DELETE FROM queue_scheduled WHERE id = $1 \
                         RETURNING kind, queue, payload, max_attempts, \
                                   unique_key, uniqueness, enqueued_at \
                     ) \
                     INSERT INTO {table} \
                         (kind, queue, payload, available_at, max_attempts, \
                          unique_key, uniqueness, enqueued_at) \
                     SELECT kind, queue, payload, now(), max_attempts, \
                            CASE WHEN uniqueness = 'until_success' \
                                 THEN unique_key ELSE NULL END, \
                            uniqueness, enqueued_at \
                       FROM due \
                     ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING"
                );
                Ok(sqlx::query(AssertSqlSafe(sql))
                    .bind(n)
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
                    > 0)
            }
        }
    }

    async fn discard(&self, id: &JobId) -> Result<bool, BackendError> {
        let table = self.config.table.name();
        match parse_ref(id) {
            None => Ok(false),
            Some(Ref::Live(n)) => {
                let sql = format!(
                    "DELETE FROM {table} \
                      WHERE id = $1 \
                        AND (lease_token IS NULL OR available_at <= now())"
                );
                Ok(sqlx::query(AssertSqlSafe(sql))
                    .bind(n)
                    .execute(&self.pool)
                    .await
                    .map_err(err)?
                    .rows_affected()
                    > 0)
            }
            Some(Ref::Scheduled(n)) => Ok(sqlx::query("DELETE FROM queue_scheduled WHERE id = $1")
                .bind(n)
                .execute(&self.pool)
                .await
                .map_err(err)?
                .rows_affected()
                > 0),
        }
    }

    async fn requeue(&self, id: &DeadId) -> Result<Option<JobId>, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(None);
        };
        let table = self.config.table.name();
        // Two statements in one transaction rather than one CTE, because
        // the delete must not happen when the insert does not. A unique key
        // the live table already holds makes the insert do nothing, and the
        // dead row has to stay: the operator asked for one copy of the job
        // and there is one, but their corpse is still the only record of
        // why this one died.
        let mut tx = self.pool.begin().await.map_err(err)?;
        let sql = format!(
            "INSERT INTO {table} \
                 (kind, queue, payload, available_at, attempts, max_attempts, \
                  unique_key, uniqueness, enqueued_at) \
             SELECT kind, queue, payload, now(), 0, \
                    coalesce(max_attempts, $2), unique_key, \
                    CASE WHEN unique_key IS NULL THEN 'none' ELSE 'until_success' END, \
                    now() \
               FROM queue_dead WHERE id = $1 \
             ON CONFLICT (unique_key) WHERE unique_key IS NOT NULL DO NOTHING \
             RETURNING id"
        );
        let new_id: Option<i64> = sqlx::query_scalar(AssertSqlSafe(sql))
            .bind(n)
            .bind(REQUEUE_DEFAULT_ATTEMPTS)
            .fetch_optional(&mut *tx)
            .await
            .map_err(err)?;
        let Some(new_id) = new_id else {
            tx.rollback().await.map_err(err)?;
            return Ok(None);
        };
        sqlx::query("DELETE FROM queue_dead WHERE id = $1")
            .bind(n)
            .execute(&mut *tx)
            .await
            .map_err(err)?;
        tx.commit().await.map_err(err)?;
        Ok(Some(live_id(new_id)))
    }

    async fn forget(&self, id: &DeadId) -> Result<bool, BackendError> {
        let Ok(n) = id.as_str().parse::<i64>() else {
            return Ok(false);
        };
        Ok(sqlx::query("DELETE FROM queue_dead WHERE id = $1")
            .bind(n)
            .execute(&self.pool)
            .await
            .map_err(err)?
            .rows_affected()
            > 0)
    }

    async fn finished(
        &self,
        since: SystemTime,
        bucket: Bucket,
    ) -> Result<Vec<Finished>, BackendError> {
        if !self.config.archive {
            return Err(BackendError::Unsupported("finished"));
        }
        // The window is a duration and not a timestamp, for the reason
        // every other statement in this adapter takes one: the client's
        // clock and the server's are not the same clock.
        let back = SystemTime::now()
            .duration_since(since)
            .unwrap_or(Duration::ZERO)
            .as_secs_f64();
        // `finished_at` is the partition key, so the range prunes to the
        // days it touches and the rest of the archive is never opened.
        let rows = sqlx::query(
            "SELECT date_trunc($1, finished_at) AS at, kind, queue, count(*) AS n \
               FROM queue_archive \
              WHERE finished_at >= now() - make_interval(secs => $2) \
              GROUP BY 1, 2, 3 \
              ORDER BY 1",
        )
        .bind(bucket.as_str())
        .bind(back)
        .fetch_all(&self.pool)
        .await
        .map_err(err)?;
        Ok(rows
            .iter()
            .map(|r| Finished {
                at: when(r.get("at")),
                kind: r.get("kind"),
                queue: r.get("queue"),
                count: r.get::<i64, _>("n").max(0) as u64,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_remembers_which_table_it_came_from() {
        assert!(matches!(
            parse_ref(&JobId("12".into())),
            Some(Ref::Live(12))
        ));
        assert!(matches!(
            parse_ref(&JobId("s12".into())),
            Some(Ref::Scheduled(12))
        ));
        // Anything else is a pasted URL, not a row.
        assert!(parse_ref(&JobId("".into())).is_none());
        assert!(parse_ref(&JobId("s".into())).is_none());
        assert!(parse_ref(&JobId("12; DROP TABLE".into())).is_none());
    }

    #[test]
    fn the_four_states_do_not_overlap() {
        // Spelled out rather than asserted over SQL: these four predicates
        // are the only definition of a job's state in the crate, and a pair
        // that overlapped would double-count a row on the dashboard.
        let all: Vec<&str> = View::ALL.iter().map(|v| predicate(*v)).collect();
        for (i, a) in all.iter().enumerate() {
            for b in all.iter().skip(i + 1) {
                assert_ne!(a, b);
            }
        }
        assert!(predicate(View::Ready).contains("available_at <= now()"));
        for v in [View::Running, View::Retrying, View::Scheduled] {
            assert!(predicate(v).contains("available_at > now()"));
        }
    }
}
