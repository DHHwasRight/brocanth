//! An excluded column stays out of the admin: not searched, not sorted,
//! not a label, and not in what the journal hears.
//!
//! The search count is the case that matters: if `?q=` matched an excluded
//! column, the row count would tell a reader what the column holds.

use std::sync::{Arc, Mutex};

use brocanth_admin::admin::Response;
use brocanth_admin::journal::AdminWrite;
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission};
use brocanth_test::Html;

type Heard = Arc<Mutex<Vec<Vec<String>>>>;

fn admin() -> (Admin, Heard) {
    let heard: Heard = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&heard);
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .with_policy(AllowAllUnsafeForDevelopment)
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .journal(move |w: &AdminWrite<'_>| {
                log.lock()
                    .unwrap()
                    .push(w.fields.iter().map(|f| f.name.clone()).collect());
            })
            // `name` is the text column a label falls back to first.
            .model("user", |m| m.exclude(["email", "name"])),
    );
    (admin, heard)
}

fn send(admin: &Admin, method: &str, path: &str, query: &str) -> Response {
    let form = Submission::new();
    block_on(admin.dispatch(Principal::anonymous(), method, path, query, &form))
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

fn list(admin: &Admin, query: &str) -> Html {
    Html::new(
        send(admin, "GET", "/admin/user", query)
            .html()
            .expect("a page"),
    )
}

#[test]
fn a_search_does_not_match_an_excluded_column() {
    let (admin, _) = admin();
    // `grace@example.com` is only in `email`; `Wrote` only in `bio`.
    let rows = |page: &Html| page.tags("tr").len();
    let all = rows(&list(&admin, ""));
    let none = rows(&list(&admin, "q=Nobody+at+all"));
    assert!(all > none, "the fixture lists users");
    assert_eq!(
        rows(&list(&admin, "q=grace%40example")),
        none,
        "an excluded column is not searched"
    );
    let shown = list(&admin, "q=Wrote");
    shown.assert_text("Wrote the first program.");
}

#[test]
fn a_sort_on_an_excluded_column_is_ignored() {
    let (admin, _) = admin();
    let plain = list(&admin, "").text();
    let sorted = list(&admin, "sort=name&dir=desc").text();
    assert_eq!(
        plain, sorted,
        "the order does not follow the excluded column"
    );
}

#[test]
fn an_excluded_column_names_no_record() {
    let (admin, _) = admin();
    let page = Html::new(
        send(&admin, "GET", "/admin/user/2", "")
            .html()
            .expect("a page"),
    );
    assert!(!page.text().contains("Grace Hopper"), "{}", page.text());
}

#[test]
fn a_delete_does_not_journal_an_excluded_column() {
    let (admin, heard) = admin();
    let removed = send(&admin, "POST", "/admin/user/2/delete", "");
    assert_eq!(removed.status(), 303);
    let heard = heard.lock().unwrap().clone();
    assert_eq!(heard.len(), 1);
    assert!(heard[0].contains(&"role".to_string()), "{heard:?}");
    assert!(
        !heard[0].iter().any(|f| f == "email" || f == "name"),
        "{heard:?}"
    );
}
