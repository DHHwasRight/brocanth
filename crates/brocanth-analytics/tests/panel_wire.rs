//! The analytics tab's data contract (docs/backoffice.md §7.6): each call
//! and answer survives JSON, the screen is the same in this process and
//! over the wire, and the data side checks the grant of each call. Over the
//! memory store, and over `PgStore` when `BROCANTH_TEST_DATABASE_URL` names
//! a server.
//!
//! The wire is `remote::Loopback`: it encodes each call as the backoffice
//! wire does, and checks it with the same `remote::authorize`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use brocanth_admin::panel::remote::{Grants, Loopback};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_analytics::admin::{
    AnalyticsAnswer, AnalyticsCall, AnalyticsOverview, AnalyticsPanel,
};
use brocanth_analytics::{
    Day, Dimension, Line, MemoryStore, PgStore, Recorder, Report, Store, View,
};
use brocanth_db::sqlx::{self, AssertSqlSafe};
use brocanth_db::{Db, DbConfig};

/// A member of staff, as the loopback reads their grants.
struct Who(Grants);

fn who(permissions: &[&str]) -> Who {
    Who(Grants {
        superuser: false,
        permissions: permissions.iter().map(|p| p.to_string()).collect(),
    })
}

fn grants(principal: &Principal<'_>) -> Grants {
    principal
        .downcast::<Who>()
        .map(|who| who.0.clone())
        .unwrap_or_default()
}

fn view(site: &str, day: i32, visitor: i64, path: &str) -> View {
    View {
        site: site.to_string(),
        day: Day(day),
        visitor,
        path: path.to_string(),
        query: String::new(),
        referrer: String::new(),
        browser: "Firefox".to_string(),
        os: "Linux".to_string(),
        device: "desktop".to_string(),
        country: String::new(),
    }
}

/// A recorder on day 20 000 over `store`, with views of two sites.
async fn recorder(store: Arc<dyn Store>) -> Recorder {
    store
        .write(&[
            view("site", 20_000, 1, "/"),
            view("site", 19_999, 2, "/pricing"),
            view("docs.example", 20_000, 7, "/guide"),
        ])
        .await
        .expect("written");
    let recorder = Recorder::new(
        "site",
        760,
        Arc::new(|| UNIX_EPOCH + Duration::from_secs(20_000 * 86_400 + 3_600)),
    );
    recorder.attach(store);
    recorder
}

/// The application's admin, whose tab serves itself, and the backoffice's,
/// whose tab reads it through the loopback. The backoffice's own recorder
/// has no store: what its tab shows came over the wire.
fn both(recorder: Recorder) -> (Admin, Admin) {
    let allow = || policy_fn(|_: &Principal<'_>, _: Request<'_>| true);
    let application = AdminConfig::new().panel(AnalyticsPanel::new(recorder));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(Loopback::new(grants), |wire, (tab, serve)| {
            wire.panel(tab, serve)
        })
        .shared();
    let local = Admin::new(Arc::new(Fixture::new()), application.with_policy(allow()));
    let empty = Recorder::new("elsewhere", 760, Arc::new(std::time::SystemTime::now));
    let remote = Admin::new(
        Arc::new(Fixture::new()),
        AdminConfig::new()
            .panel(AnalyticsPanel::new(empty))
            .panels_through(wire)
            .with_policy(allow()),
    );
    (local, remote)
}

async fn get(admin: &Admin, who: &Who, query: &str) -> String {
    let page = admin
        .dispatch(
            Principal::new(who),
            "GET",
            "/admin/analytics",
            query,
            &Submission::new(),
        )
        .await
        .expect("a page");
    assert_eq!(page.status(), 200, "{query}");
    page.html().unwrap_or_default().to_string()
}

#[test]
fn each_call_and_answer_survives_json() {
    let calls = [
        AnalyticsCall::Overview,
        AnalyticsCall::Report {
            site: "site".into(),
            from: Day(19_990),
            to: Day(20_000),
        },
    ];
    for call in calls {
        let text = serde_json::to_string(&call).expect("it encodes");
        assert_eq!(serde_json::from_str::<AnalyticsCall>(&text).unwrap(), call);
    }
    let answers = [
        AnalyticsAnswer::Overview(AnalyticsOverview {
            site: "site".into(),
            sites: vec!["site".into(), "docs.example".into()],
            today: Day(20_000),
            written: 3,
            pending: 1,
            dropped: 0,
            last_error: Some("down".into()),
        }),
        AnalyticsAnswer::Report(Report {
            views: 2,
            visitors: 2,
            days: vec![(Day(20_000), 2, 2)],
            breakdowns: vec![(
                Dimension::Path,
                vec![Line {
                    label: "/".into(),
                    views: 2,
                    visitors: 2,
                }],
            )],
        }),
    ];
    for answer in answers {
        let text = serde_json::to_string(&answer).expect("it encodes");
        assert_eq!(
            serde_json::from_str::<AnalyticsAnswer>(&text).unwrap(),
            answer
        );
    }
}

async fn the_screen_is_the_same_and_the_grant_is_checked(store: Arc<dyn Store>) {
    let (local, remote) = both(recorder(store).await);
    let reader = who(&["analytics.read"]);
    for query in [
        "",
        "days=7",
        "site=docs.example",
        "from=2024-10-03&to=2024-10-04",
    ] {
        assert_eq!(
            get(&local, &reader, query).await,
            get(&remote, &reader, query).await,
            "{query}"
        );
    }
    let html = get(&remote, &reader, "site=docs.example").await;
    assert!(html.contains("/guide"), "the report crossed the wire");
    let refused = get(&remote, &who(&[]), "").await;
    assert!(
        refused.contains("could not be read") && refused.contains("analytics.read"),
        "{refused}"
    );
}

#[tokio::test]
async fn over_the_memory_store() {
    the_screen_is_the_same_and_the_grant_is_checked(Arc::new(MemoryStore::new())).await;
}

/// The data side holds a range to `MAX_RANGE` days, whatever the call says.
#[tokio::test]
async fn the_data_side_refuses_a_range_too_long() {
    let (_, serve) = AdminConfig::new()
        .panel(AnalyticsPanel::new(
            recorder(Arc::new(MemoryStore::new())).await,
        ))
        .panel_serves()
        .pop()
        .expect("the tab serves itself");
    let call = serde_json::to_string(&AnalyticsCall::Report {
        site: "site".into(),
        from: Day(10_000),
        to: Day(20_000),
    })
    .unwrap();
    let refused = serve
        .serve(&Principal::anonymous(), &call)
        .await
        .expect_err("ten thousand days");
    assert!(refused.contains("shorter range"), "{refused}");
}

#[tokio::test]
async fn over_postgres() {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(url) = [
        "BROCANTH_ANALYTICS_TEST_POSTGRES",
        "BROCANTH_TEST_DATABASE_URL",
    ]
    .iter()
    .find_map(|v| std::env::var(v).ok().filter(|u| !u.trim().is_empty())) else {
        eprintln!("!! SKIPPED over_postgres: set BROCANTH_TEST_DATABASE_URL");
        return;
    };
    let schema = format!(
        "brocanth_analytics_wire{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let admin = Db::connect(&DbConfig::new(&url)).await.expect("connect");
    let exec = |sql: String| {
        let admin = admin.clone();
        async move {
            sqlx::query(AssertSqlSafe(sql))
                .execute(admin.pool())
                .await
                .expect("a statement");
        }
    };
    exec(format!("CREATE SCHEMA {schema}")).await;
    let mut cfg = DbConfig::new(&url);
    cfg.search_path = vec![schema.clone()];
    let db = Db::connect(&cfg).await.expect("connect");
    let store = PgStore::new(db.clone());
    store.migrate().await.expect("the schema");
    the_screen_is_the_same_and_the_grant_is_checked(Arc::new(store)).await;
    db.close().await;
    exec(format!("DROP SCHEMA {schema} CASCADE")).await;
}
