//! The archive's partitions.
//!
//! The archive is append only and strictly ordered by `finished_at`, which
//! is the only shape where `DROP PARTITION` is the right cleanup. A live
//! queue is not that shape: a partition cannot be dropped while one row in
//! it is still retrying, and a retry can be days after the enqueue.

use chrono::{Duration as Days, NaiveDate, Utc};
use sqlx::{AssertSqlSafe, PgPool};

fn name(day: NaiveDate) -> String {
    format!("queue_archive_{}", day.format("%Y%m%d"))
}

/// Create today's partition and the next `ahead` days.
///
/// Call it at boot and on a timer. A missing partition makes an insert fail,
/// so the margin should be more than a day.
///
/// Safe to run from every worker at once. `CREATE TABLE IF NOT EXISTS` is
/// not atomic against another session doing the same thing: both see the
/// table missing, both create it, and the loser gets a duplicate-object
/// error rather than the silence the `IF NOT EXISTS` promises. Losing that
/// race is the expected outcome on a fleet, so it is not an error here.
pub async fn ensure_archive_partitions(pool: &PgPool, ahead: i64) -> Result<(), sqlx::Error> {
    let today = Utc::now().date_naive();
    for n in 0..=ahead {
        let from = today + Days::days(n);
        let to = from + Days::days(1);
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {} PARTITION OF queue_archive \
             FOR VALUES FROM ('{}') TO ('{}')",
            name(from),
            from,
            to
        );
        match sqlx::query(AssertSqlSafe(sql.clone())).execute(pool).await {
            Ok(_) => {}
            Err(e) if lost_the_race(&e) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Whether this error means another session already did the work.
///
/// `42P07` is duplicate_table. `23505` is the unique violation Postgres
/// raises from the catalogue itself when two sessions insert the same
/// `pg_type` row at the same moment, which is what a lost `CREATE TABLE`
/// race looks like from underneath.
fn lost_the_race(e: &sqlx::Error) -> bool {
    matches!(
        e.as_database_error().and_then(|d| d.code()).as_deref(),
        Some("42P07") | Some("23505")
    )
}

/// Drop every archive partition that ends before `day`.
///
/// This is the whole retention policy: one `DROP TABLE` per day, no dead
/// tuples, no vacuum, no `DELETE` holding a lock on a hot table.
pub async fn drop_archive_partitions_before(
    pool: &PgPool,
    day: NaiveDate,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<String> = sqlx::query_scalar(
        "SELECT c.relname::text \
           FROM pg_class c \
           JOIN pg_inherits i ON i.inhrelid = c.oid \
           JOIN pg_class p ON p.oid = i.inhparent \
          WHERE p.relname = 'queue_archive'",
    )
    .fetch_all(pool)
    .await?;

    let cutoff = format!("queue_archive_{}", day.format("%Y%m%d"));
    let mut dropped = Vec::new();
    for table in rows {
        if table < cutoff {
            sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
                .execute(pool)
                .await?;
            dropped.push(table);
        }
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_names_sort_the_way_dates_do() {
        // The drop compares names as strings, so the format has to make
        // that the same order as the dates.
        let a = name(NaiveDate::from_ymd_opt(2026, 1, 9).unwrap());
        let b = name(NaiveDate::from_ymd_opt(2026, 1, 10).unwrap());
        let c = name(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        assert!(a < b && b < c, "{a} {b} {c}");
    }
}
