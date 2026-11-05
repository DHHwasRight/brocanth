//! [`PgStore`]: page views in two tables of the application's own database.
//!
//! Plain SQL through the pool the `db` module opened, not sea-query: the
//! batch insert is one `INSERT … SELECT FROM UNNEST(…)` with one array per
//! column, which sea-query has no builder for, and the reports group by a
//! column picked from a closed enum. Every value is a bound parameter.

use async_trait::async_trait;
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::Db;

use crate::day::Day;
use crate::store::{Dimension, Line, Report, Store, StoreError, View, TOP};

/// Page views in Postgres.
#[derive(Clone, Debug)]
pub struct PgStore {
    db: Db,
}

impl PgStore {
    /// The two tables and their index, idempotent.
    ///
    /// `day` is a `date` and the only time on a row. `visitor` is the daily
    /// hash; it is not a key and joins nothing. The salts table holds one
    /// row per day and is emptied of every day before today, once a day.
    pub const SCHEMA: &'static str = "\
CREATE TABLE IF NOT EXISTS brocanth_analytics_salts (
    day  date  PRIMARY KEY,
    salt bytea NOT NULL
);
CREATE TABLE IF NOT EXISTS brocanth_analytics_views (
    id       bigserial PRIMARY KEY,
    site     text   NOT NULL,
    day      date   NOT NULL,
    visitor  bigint NOT NULL,
    path     text   NOT NULL,
    query    text   NOT NULL DEFAULT '',
    referrer text   NOT NULL DEFAULT '',
    browser  text   NOT NULL,
    os       text   NOT NULL,
    device   text   NOT NULL,
    country  text   NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS brocanth_analytics_views_site_day
    ON brocanth_analytics_views (site, day);
";

    /// A store over `db`. Creates nothing: see [`PgStore::migrate`].
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Runs [`PgStore::SCHEMA`].
    ///
    /// # Errors
    /// The database refused a statement.
    pub async fn migrate(&self) -> Result<(), StoreError> {
        for statement in Self::SCHEMA
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sqlx::query(AssertSqlSafe(statement.to_string()))
                .execute(self.db.pool())
                .await
                .map_err(failed)?;
        }
        Ok(())
    }
}

fn failed(e: sqlx::Error) -> StoreError {
    StoreError::new(format!("analytics store: {e}"))
}

/// `day` is stored as a `date` and travels as days since 1970, so this
/// crate needs no date library: `DATE '1970-01-01' + n` and
/// `day - DATE '1970-01-01'` are exact.
const EPOCH: &str = "DATE '1970-01-01'";

#[async_trait]
impl Store for PgStore {
    async fn salt(&self, day: Day) -> Result<[u8; 32], StoreError> {
        let fresh = crate::recorder::fresh_salt()?;
        // Two instances may race for a new day. `DO NOTHING` keeps the
        // first salt, and both read it back.
        sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO brocanth_analytics_salts (day, salt) VALUES ({EPOCH} + $1, $2) \
             ON CONFLICT (day) DO NOTHING"
        )))
        .bind(day.0)
        .bind(fresh.to_vec())
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        let (salt,): (Vec<u8>,) = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT salt FROM brocanth_analytics_salts WHERE day = {EPOCH} + $1"
        )))
        .bind(day.0)
        .fetch_one(self.db.pool())
        .await
        .map_err(failed)?;
        <[u8; 32]>::try_from(salt.as_slice())
            .map_err(|_| StoreError::new("analytics store: a salt is not 32 bytes"))
    }

    async fn write(&self, views: &[View]) -> Result<(), StoreError> {
        for chunk in views.chunks(1_000) {
            let col = |f: fn(&View) -> String| chunk.iter().map(f).collect::<Vec<String>>();
            sqlx::query(AssertSqlSafe(format!(
                "INSERT INTO brocanth_analytics_views \
                 (site, day, visitor, path, query, referrer, browser, os, device, country) \
                 SELECT site, {EPOCH} + day, visitor, path, query, referrer, browser, os, device, country \
                 FROM UNNEST($1::text[], $2::int4[], $3::int8[], $4::text[], $5::text[], \
                 $6::text[], $7::text[], $8::text[], $9::text[], $10::text[]) \
                 AS v(site, day, visitor, path, query, referrer, browser, os, device, country)"
            )))
            .bind(col(|v| v.site.clone()))
            .bind(chunk.iter().map(|v| v.day.0).collect::<Vec<i32>>())
            .bind(chunk.iter().map(|v| v.visitor).collect::<Vec<i64>>())
            .bind(col(|v| v.path.clone()))
            .bind(col(|v| v.query.clone()))
            .bind(col(|v| v.referrer.clone()))
            .bind(col(|v| v.browser.clone()))
            .bind(col(|v| v.os.clone()))
            .bind(col(|v| v.device.clone()))
            .bind(col(|v| v.country.clone()))
            .execute(self.db.pool())
            .await
            .map_err(failed)?;
        }
        Ok(())
    }

    async fn forget(&self, today: Day, keep_from: Option<Day>) -> Result<(), StoreError> {
        sqlx::query(AssertSqlSafe(format!(
            "DELETE FROM brocanth_analytics_salts WHERE day < {EPOCH} + $1"
        )))
        .bind(today.0)
        .execute(self.db.pool())
        .await
        .map_err(failed)?;
        if let Some(keep_from) = keep_from {
            sqlx::query(AssertSqlSafe(format!(
                "DELETE FROM brocanth_analytics_views WHERE day < {EPOCH} + $1"
            )))
            .bind(keep_from.0)
            .execute(self.db.pool())
            .await
            .map_err(failed)?;
        }
        Ok(())
    }

    async fn sites(&self) -> Result<Vec<String>, StoreError> {
        let rows: Vec<(String,)> =
            sqlx::query_as("SELECT DISTINCT site FROM brocanth_analytics_views ORDER BY site")
                .fetch_all(self.db.pool())
                .await
                .map_err(failed)?;
        Ok(rows.into_iter().map(|(site,)| site).collect())
    }

    async fn report(&self, site: &str, from: Day, to: Day) -> Result<Report, StoreError> {
        let range = format!("site = $1 AND day BETWEEN {EPOCH} + $2 AND {EPOCH} + $3");
        let (views, visitors): (i64, i64) = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT count(*), count(DISTINCT (day, visitor)) \
             FROM brocanth_analytics_views WHERE {range}"
        )))
        .bind(site)
        .bind(from.0)
        .bind(to.0)
        .fetch_one(self.db.pool())
        .await
        .map_err(failed)?;
        let days: Vec<(i32, i64, i64)> = sqlx::query_as(AssertSqlSafe(format!(
            "SELECT day - {EPOCH}, count(*), count(DISTINCT visitor) \
             FROM brocanth_analytics_views WHERE {range} GROUP BY day ORDER BY day"
        )))
        .bind(site)
        .bind(from.0)
        .bind(to.0)
        .fetch_all(self.db.pool())
        .await
        .map_err(failed)?;
        let mut breakdowns = Vec::new();
        for dimension in Dimension::ALL {
            // A column name from a closed enum, never from input.
            let column = dimension.column();
            let lines: Vec<(String, i64, i64)> = sqlx::query_as(AssertSqlSafe(format!(
                "SELECT {column}, count(*), count(DISTINCT (day, visitor)) \
                 FROM brocanth_analytics_views WHERE {range} AND {column} <> '' \
                 GROUP BY {column} ORDER BY 2 DESC, 1 LIMIT {TOP}"
            )))
            .bind(site)
            .bind(from.0)
            .bind(to.0)
            .fetch_all(self.db.pool())
            .await
            .map_err(failed)?;
            breakdowns.push((
                dimension,
                lines
                    .into_iter()
                    .map(|(label, views, visitors)| Line {
                        label,
                        views,
                        visitors,
                    })
                    .collect(),
            ));
        }
        Ok(Report {
            views,
            visitors,
            days: days
                .into_iter()
                .map(|(day, views, visitors)| (Day(day), views, visitors))
                .collect(),
            breakdowns,
        })
    }
}
