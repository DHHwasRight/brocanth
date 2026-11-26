//! The cache tab's data contract (docs/backoffice.md §7.6): each call and
//! answer survives JSON, the tab answers the same in this process and over
//! the wire, and the data side checks the grant of each call.
//!
//! The wire is `remote::Loopback`: it encodes each call as the backoffice
//! wire does, and checks it with the same `remote::authorize`.

#![cfg(feature = "admin")]

use std::sync::Arc;
use std::time::Duration;

use brocanth_admin::panel::remote::{Grants, Loopback};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};
use brocanth_cache::admin::{CacheAnswer, CacheCall, CachePanel, CacheView};
use brocanth_cache::{Cache, Key, MemoryCache, Namespace};

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

fn pages() -> Namespace {
    Namespace::new("pages").expect("a namespace")
}

async fn filled() -> Arc<MemoryCache> {
    let cache = Arc::new(MemoryCache::new().with_max_value_bytes(2048));
    for key in ["a", "b"] {
        let key = Key::new(pages(), key).expect("a key");
        cache
            .set(&key, b"x", Duration::from_secs(60))
            .await
            .expect("stored");
    }
    cache
}

fn panel(cache: Arc<MemoryCache>) -> CachePanel {
    CachePanel::new(cache).namespace(pages())
}

/// The application's admin, whose tab serves itself, and the backoffice's,
/// whose tab reads it through the loopback.
fn both(cache: Arc<MemoryCache>) -> (Admin, Admin) {
    let allow = || policy_fn(|_: &Principal<'_>, _: Request<'_>| true);
    let application = AdminConfig::new().panel(panel(cache.clone()));
    let wire = application
        .panel_serves()
        .into_iter()
        .fold(Loopback::new(grants), |wire, (tab, serve)| {
            wire.panel(tab, serve)
        })
        .shared();
    let local = Admin::new(Arc::new(Fixture::blog()), application.with_policy(allow()));
    // The backoffice's own cache is empty: what its tab shows came over
    // the wire.
    let remote = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .panel(panel(Arc::new(MemoryCache::new())))
            .panels_through(wire)
            .with_policy(allow()),
    );
    (local, remote)
}

async fn get(admin: &Admin, who: &Who) -> (u16, Option<String>) {
    let page = admin
        .dispatch(
            Principal::new(who),
            "GET",
            "/admin/cache",
            "",
            &Submission::new(),
        )
        .await
        .expect("a page");
    (page.status(), page.html().map(str::to_string))
}

async fn post(admin: &Admin, who: &Who, path: &str) -> Result<Option<String>, String> {
    admin
        .dispatch(Principal::new(who), "POST", path, "", &Submission::new())
        .await
        .map(|page| page.location().map(str::to_string))
        .map_err(|e| e.to_string())
}

#[test]
fn each_call_and_answer_survives_json() {
    let calls = [
        CacheCall::Read,
        CacheCall::Clear {
            namespace: "pages".into(),
        },
        CacheCall::Purge { limit: 10 },
    ];
    for call in calls {
        let text = serde_json::to_string(&call).expect("it encodes");
        assert_eq!(serde_json::from_str::<CacheCall>(&text).unwrap(), call);
    }
    let answers = [
        CacheAnswer::View(CacheView {
            max_value_bytes: 2048,
        }),
        CacheAnswer::Removed(3),
    ];
    for answer in answers {
        let text = serde_json::to_string(&answer).expect("it encodes");
        assert_eq!(serde_json::from_str::<CacheAnswer>(&text).unwrap(), answer);
    }
}

#[tokio::test]
async fn the_tab_answers_the_same_in_this_process_and_over_the_wire() {
    let editor = who(&["cache.edit"]);
    let (local, _) = both(filled().await);
    let (_, remote) = both(filled().await);
    assert_eq!(get(&local, &editor).await, get(&remote, &editor).await);
    let (status, html) = get(&remote, &editor).await;
    assert_eq!(status, 200);
    assert!(html.is_some_and(|html| html.contains("2 KiB")));
    for path in ["/admin/cache/clear/pages", "/admin/cache/purge"] {
        assert_eq!(
            post(&local, &editor, path).await,
            post(&remote, &editor, path).await,
            "{path}"
        );
    }
    let cleared = post(&remote, &editor, "/admin/cache/clear/pages").await;
    assert_eq!(
        cleared,
        Ok(Some("/admin/cache?cleared=pages&rows=0".to_string())),
        "the first clear emptied it"
    );
}

#[tokio::test]
async fn the_data_side_checks_the_grant_of_each_call() {
    let cache = filled().await;
    let (_, remote) = both(cache.clone());
    let reader = who(&["cache.read"]);
    let nobody = who(&[]);

    assert_eq!(get(&remote, &reader).await.0, 200);
    let refused = post(&remote, &reader, "/admin/cache/clear/pages").await;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("cache.edit")),
        "{refused:?}"
    );
    let key = Key::new(pages(), "a").expect("a key");
    assert!(
        cache.get(&key).await.expect("a read").is_some(),
        "a reader's clear emptied nothing"
    );
    let refused = post(&remote, &reader, "/admin/cache/purge").await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(
        remote
            .dispatch(
                Principal::new(&nobody),
                "GET",
                "/admin/cache",
                "",
                &Submission::new()
            )
            .await
            .is_err(),
        "nobody reads the cache"
    );
}

/// A call made by hand, past the tab: the data side refuses a namespace the
/// tab did not declare, whoever sends it.
#[tokio::test]
async fn the_data_side_refuses_an_undeclared_namespace() {
    let (_, serve) = AdminConfig::new()
        .panel(panel(filled().await))
        .panel_serves()
        .pop()
        .expect("the tab serves itself");
    let editor = who(&["cache.edit"]);
    let call = serde_json::to_string(&CacheCall::Clear {
        namespace: "sessions".into(),
    })
    .unwrap();
    let refused = serve
        .serve(&Principal::new(&editor), &call)
        .await
        .expect_err("an undeclared namespace");
    assert!(refused.contains("declares no namespace"), "{refused}");
}
