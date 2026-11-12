//! A foreign-key picker keeps the value the record holds.
//!
//! The picker lists at most `choice_limit` targets. A record that points
//! past them must still show its own target selected, or a save of another
//! field writes the first option, or NULL, into the relation.

use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission};
use brocanth_test::Html;

fn admin() -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .with_policy(AllowAllUnsafeForDevelopment)
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .model("user", |m| m.choice_limit(1)),
    )
}

fn page(admin: &Admin, path: &str) -> Html {
    let form = Submission::new();
    let answer = block_on(admin.dispatch(Principal::anonymous(), "GET", path, "", &form))
        .unwrap_or_else(|e| panic!("GET {path}: {e}"));
    Html::new(answer.html().expect("a page"))
}

fn options(html: &Html) -> Vec<(String, bool)> {
    html.within("select", "name", "author_id")
        .tags("option")
        .iter()
        .map(|o| {
            (
                o.attr("value").unwrap_or_default().to_string(),
                o.has_attr("selected"),
            )
        })
        .collect()
}

#[test]
fn the_stored_target_is_offered_and_selected_past_the_limit() {
    let admin = admin();
    // Post 2 is Grace's, user 2: past a limit of one.
    let edit = options(&page(&admin, "/admin/post/2/edit"));
    assert!(
        edit.contains(&("2".to_string(), true)),
        "the stored author is selected: {edit:?}"
    );
    assert!(
        edit.contains(&("1".to_string(), false)),
        "the first choice in key order is still there: {edit:?}"
    );
}

#[test]
fn the_choices_are_in_key_order() {
    let admin = admin();
    let new = options(&page(&admin, "/admin/post/new"));
    let values: Vec<&str> = new
        .iter()
        .map(|(v, _)| v.as_str())
        .filter(|v| !v.is_empty())
        .collect();
    assert_eq!(values, ["1"], "one choice, the lowest key");
}
