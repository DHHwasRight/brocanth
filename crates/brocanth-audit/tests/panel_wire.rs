//! The history tab's data contract (docs/backoffice.md §7.6): the query
//! and the view survive JSON, the tab is the same in this process and over
//! the wire, and the data side checks `history.read` on each call. The
//! values the person may not read stay hidden on the backoffice's side.
//!
//! The wire is `remote::Loopback`: it encodes each call as the backoffice
//! wire does, and checks it with the same `remote::authorize`.

use std::sync::Arc;

use brocanth_admin::panel::remote::{Grants, Loopback};
use brocanth_admin::policy::{policy_fn, Action, Principal, Request};
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Submission};
use brocanth_audit::admin::{AuditPanel, HistoryView};
use brocanth_audit::{Entry, Field, Kind, MemoryStore, Query, Trail};
use brocanth_events::{ChangeSink, RowChange};

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

/// The admin's own policy: the tab, and `post` records, are open; a `user`
/// record's values are not.
fn config() -> AdminConfig {
    AdminConfig::new()
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(policy_fn(|_: &Principal<'_>, r: Request<'_>| {
            r.action != Action::ViewDetail || r.model == "post"
        }))
}

/// The application's admin, whose tab serves itself, and the backoffice's,
/// whose tab reads the trail through the loopback. The backoffice's own
/// trail is empty.
fn both(trail: &Trail) -> (Admin, Admin) {
    let application = config().panel(AuditPanel::new(trail.clone()));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(Loopback::new(grants), |wire, (tab, serve)| {
            wire.panel(tab, serve)
        })
        .shared();
    let local = Admin::new(Arc::new(Fixture::blog()), application);
    let empty = Trail::with_store(Arc::new(MemoryStore::new()), 0);
    let remote = Admin::new(
        Arc::new(Fixture::blog()),
        config().panel(AuditPanel::new(empty)).panels_through(wire),
    );
    (local, remote)
}

async fn get(admin: &Admin, who: &Who, query: &str) -> String {
    let page = admin
        .dispatch(
            Principal::new(who),
            "GET",
            "/admin/history",
            query,
            &Submission::new(),
        )
        .await
        .expect("a page");
    assert_eq!(page.status(), 200, "{query}");
    page.html().unwrap_or_default().to_string()
}

#[test]
fn the_query_and_the_view_survive_json() {
    let query = Query {
        table: Some("post".into()),
        row: Some("7".into()),
        before: Some(40),
        limit: 50,
    };
    let text = serde_json::to_string(&query).unwrap();
    assert_eq!(serde_json::from_str::<Query>(&text).unwrap(), query);
    let view = HistoryView {
        entries: Ok(vec![Entry {
            id: 3,
            at: 1_700_000_000,
            actor: Some("ada".into()),
            source: "admin".into(),
            table: "post".into(),
            row: "7".into(),
            label: "Compilers".into(),
            kind: Kind::Updated,
            fields: vec![Field {
                name: "title".into(),
                before: Some("A".into()),
                after: Some("B".into()),
            }],
        }]),
        last_error: Some("down".into()),
        dropped: 2,
        durable: false,
    };
    let text = serde_json::to_string(&view).unwrap();
    assert_eq!(serde_json::from_str::<HistoryView>(&text).unwrap(), view);
}

#[tokio::test]
async fn the_tab_is_the_same_over_the_wire_and_needs_its_read_grant() {
    let trail = Trail::with_store(Arc::new(MemoryStore::new()), 0);
    let title = ["title"];
    trail
        .by("bob")
        .publish(&RowChange::updated("post", 2_i64, title.as_slice()));
    trail.by("ann").publish(&RowChange::deleted("user", 4_i64));
    let (local, remote) = both(&trail);
    let reader = who(&["history.read"]);
    for query in ["", "table=post&row=2", "table=user&row=4"] {
        assert_eq!(
            get(&local, &reader, query).await,
            get(&remote, &reader, query).await,
            "{query}"
        );
    }
    let all = get(&remote, &reader, "").await;
    assert!(all.contains("bob") && all.contains("ann"), "{all}");

    let refused = get(&remote, &who(&["post.read"]), "").await;
    assert!(
        refused.contains("could not be read") && refused.contains("history.read"),
        "{refused}"
    );
}
