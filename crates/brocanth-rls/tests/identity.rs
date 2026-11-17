//! `identity(&role, &subject)` on a real Postgres: inside `transact_as`,
//! each attribute reads back through its typed `Setting`, of each kind, and
//! an absent one reads as `NULL`.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL).

mod common;

use brocanth_db::sqlx::{self, AssertSqlSafe, Row as _};
use brocanth_policy::Subject;
use brocanth_rls::identity::setting_of;
use brocanth_rls::{identity, Role};
use common::{connect, url};

/// Every attribute kind.
#[derive(brocanth_policy::Subject)]
struct Visitor {
    #[attr]
    user_id: Option<i64>,
    #[attr]
    rank: i32,
    #[attr]
    admin: bool,
    #[attr]
    name: Option<String>,
    #[attr]
    team_id: Option<uuid::Uuid>,
    #[attr]
    seen_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Not an attribute: it never reaches the database.
    #[allow(dead_code)]
    email: String,
}

/// Each attribute as the policy reads it, cast back to text in UTC, and
/// `current_user`.
async fn read_back(db: &brocanth_db::Db, me: &brocanth_db::DbIdentity) -> Vec<Option<String>> {
    let reads: Vec<String> = Visitor::ATTRS
        .iter()
        .map(|attr| format!("({})::text", setting_of(attr).get().to_sql()))
        .collect();
    let sql = format!("SELECT current_user::text, {}", reads.join(", "));
    db.transact_as(me, async |tx| {
        sqlx::query("SET LOCAL TimeZone = 'UTC'")
            .execute(tx.connection())
            .await?;
        let row = sqlx::query(AssertSqlSafe(sql.clone()))
            .fetch_one(tx.connection())
            .await?;
        Ok((0..=Visitor::ATTRS.len())
            .map(|i| row.get::<Option<String>, _>(i))
            .collect())
    })
    .await
    .unwrap_or_else(|e| panic!("{e}\n{sql}"))
}

#[tokio::test]
async fn each_kind_reads_back_through_its_typed_setting() {
    let Some(url) = url() else {
        eprintln!("!! SKIPPED: BROCANTH_TEST_DATABASE_URL is not set");
        return;
    };
    let db = connect(&url, None).await;
    let role = format!("rls_identity_{}", std::process::id());
    sqlx::query(AssertSqlSafe(format!(
        "DO $$ BEGIN CREATE ROLE \"{role}\" NOLOGIN; \
         EXCEPTION WHEN duplicate_object THEN NULL; END $$"
    )))
    .execute(db.pool())
    .await
    .expect("create the role");

    let team = uuid::Uuid::parse_str("ab000000-0000-4000-8000-000000000001").unwrap();
    let seen = chrono::DateTime::parse_from_rfc3339("2026-11-23T09:30:00.000123Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let who = Visitor {
        user_id: Some(42),
        rank: -3,
        admin: true,
        name: Some("O'Brien".to_string()),
        team_id: Some(team),
        seen_at: Some(seen),
        email: "ada@example.com".to_string(),
    };
    let got = read_back(&db, &identity(&Role::new(&role), &who)).await;
    let want: Vec<Option<String>> = [
        Some(role.as_str()),
        Some("42"),
        Some("-3"),
        Some("true"),
        Some("O'Brien"),
        Some("ab000000-0000-4000-8000-000000000001"),
        Some("2026-11-23 09:30:00.000123+00"),
    ]
    .into_iter()
    .map(|v| v.map(str::to_string))
    .collect();
    assert_eq!(got, want);

    // Anonymous: every optional key is set, blank, and reads as NULL.
    let anonymous = Visitor {
        user_id: None,
        rank: 0,
        admin: false,
        name: None,
        team_id: None,
        seen_at: None,
        email: String::new(),
    };
    let got = read_back(&db, &identity(&Role::new(&role), &anonymous)).await;
    assert_eq!(
        got,
        [
            Some(role.clone()),
            None,
            Some("0".to_string()),
            Some("false".to_string()),
            None,
            None,
            None,
        ]
    );

    // The next transaction on the connection: no role, no value.
    let row = sqlx::query("SELECT current_user::text = session_user::text, nullif(current_setting('brocanth.user_id', true), '')")
        .fetch_one(db.pool())
        .await
        .unwrap();
    assert!(row.get::<bool, _>(0));
    assert_eq!(row.get::<Option<String>, _>(1), None);

    let _ = sqlx::query(AssertSqlSafe(format!("DROP ROLE IF EXISTS \"{role}\"")))
        .execute(db.pool())
        .await;
    db.close().await;
}
