//! The flags tab in a real admin: the list, one flag's form, a save, a
//! removal, and what the policy and a bad form do.
//!
//! Asserted on what a person sees and on what the copy answers next.

use std::sync::Arc;

use brocanth_admin::admin::Response;
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Fixture, Policy, Submission};
use brocanth_flags::admin::FlagsPanel;
use brocanth_flags::{Actor, Flag, Flags, MemoryStore, Rule};
use brocanth_test::Html;

const CHECKOUT: Flag = Flag::new("new_checkout", "The checkout flow from the redesign.");
const SEARCH: Flag = Flag::new("search", "Search <b>everything</b>.");
const ALL: &[Flag] = &[CHECKOUT, SEARCH];

fn flags() -> Flags {
    Flags::with_store(Arc::new(MemoryStore::new()))
}

fn admin_with(flags: &Flags, policy: impl Policy + 'static) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .with_policy(policy)
            .at("/admin")
            .panel(FlagsPanel::new(flags.clone(), ALL)),
    )
}

fn admin(flags: &Flags) -> Admin {
    admin_with(flags, AllowAllUnsafeForDevelopment)
}

fn send(admin: &Admin, method: &str, path: &str, form: &str) -> Response {
    let form = Submission::from_urlencoded(form);
    block_on(admin.dispatch(Principal::anonymous(), method, path, "", &form))
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

fn html(response: &Response) -> Html {
    Html::new(response.html().expect("a page"))
}

#[test]
fn the_list_shows_every_declared_flag_and_its_state() {
    let flags = flags();
    block_on(flags.set("search", Rule::percent(25))).unwrap();
    let page = html(&send(&admin(&flags), "GET", "/admin/flags", ""));
    page.assert_text("new_checkout")
        .assert_text("The checkout flow from the redesign.")
        .assert_text("Off")
        .assert_text("25%")
        .assert_link_to("/admin/flags/new_checkout")
        .assert_link_to("/admin/flags/search");
}

#[test]
fn a_stored_rule_no_code_declares_is_listed_so_it_can_be_removed() {
    let flags = flags();
    block_on(flags.set("old_banner", Rule::everyone())).unwrap();
    let admin = admin(&flags);
    let page = html(&send(&admin, "GET", "/admin/flags", ""));
    page.assert_text("old_banner")
        .assert_text("Not declared in the code.");
    let removed = send(&admin, "POST", "/admin/flags/old_banner", "remove=1");
    assert_eq!(removed.status(), 303);
    assert_eq!(flags.rule("old_banner"), None);
}

#[test]
fn a_saved_form_reaches_the_copy_at_once() {
    let flags = flags();
    let admin = admin(&flags);
    let form = html(&send(&admin, "GET", "/admin/flags/new_checkout", ""));
    form.assert_form_posts_to("/admin/flags/new_checkout")
        .assert_field("percent")
        .assert_button("Save");

    let saved = send(
        &admin,
        "POST",
        "/admin/flags/new_checkout",
        "percent=+30+&actors=ada%0D%0A+bob+%0D%0A%0D%0Aada&groups=staff",
    );
    assert_eq!(saved.status(), 303);
    assert_eq!(saved.location(), Some("/admin/flags"));
    assert_eq!(
        flags.rule("new_checkout"),
        Some(Rule {
            on: false,
            percent: 30,
            // Trimmed, blank lines dropped, sorted, each once.
            actors: vec!["ada".into(), "bob".into()],
            groups: vec!["staff".into()],
        })
    );
    assert!(flags.enabled(CHECKOUT, &Actor::new("bob")));
    assert!(flags.enabled(CHECKOUT, &Actor::new("zed").in_group("staff")));
    assert!(!flags.enabled(CHECKOUT, &Actor::anonymous()));

    // The form reads back what was stored.
    let again = html(&send(&admin, "GET", "/admin/flags/new_checkout", ""));
    assert_eq!(again.field("percent").unwrap().attr("value"), Some("30"));
    again.assert_text("ada");

    // On for everyone, then removed: off again at once.
    send(&admin, "POST", "/admin/flags/new_checkout", "on=1");
    assert!(flags.enabled(CHECKOUT, &Actor::anonymous()));
    let removed = send(&admin, "POST", "/admin/flags/new_checkout", "remove=1");
    assert_eq!(removed.status(), 303);
    assert!(!flags.enabled(CHECKOUT, &Actor::anonymous()));
}

#[test]
fn a_bad_percentage_is_a_form_with_the_reason_and_changes_nothing() {
    let flags = flags();
    block_on(flags.set("search", Rule::percent(10))).unwrap();
    let admin = admin(&flags);
    for bad in ["101", "-1", "ten", "12.5"] {
        let answer = send(
            &admin,
            "POST",
            "/admin/flags/search",
            &format!("percent={bad}&on=1"),
        );
        assert_eq!(answer.status(), 200, "{bad}");
        html(&answer)
            .assert_text("The percentage must be a whole number from 0 to 100")
            .assert_text(bad);
        assert_eq!(flags.rule("search"), Some(Rule::percent(10)), "{bad}");
    }
}

#[test]
fn a_key_no_code_declares_and_no_row_holds_is_not_found() {
    let flags = flags();
    let admin = admin(&flags);
    for path in [
        "/admin/flags/typo",
        "/admin/flags/New_Checkout",
        "/admin/flags/..%2Fuser",
    ] {
        assert_eq!(send(&admin, "GET", path, "").status(), 404, "{path}");
        assert_eq!(send(&admin, "POST", path, "on=1").status(), 404, "{path}");
    }
    assert!(flags.rules().is_empty(), "a typo in a URL made no row");
    // The list itself takes no POST.
    assert_eq!(send(&admin, "POST", "/admin/flags", "on=1").status(), 404);
}

#[test]
fn user_input_is_escaped_on_the_list_and_the_form() {
    let flags = flags();
    let admin = admin(&flags);
    send(
        &admin,
        "POST",
        "/admin/flags/search",
        "actors=%3Cscript%3Ealert(1)%3C%2Fscript%3E&groups=%22%3E%3Cimg+src%3Dx%3E",
    );
    let form = send(&admin, "GET", "/admin/flags/search", "");
    let raw = form.html().unwrap();
    assert!(!raw.contains("<script>alert(1)"), "{raw}");
    assert!(!raw.contains("<img src=x>"), "{raw}");
    assert!(
        !raw.contains("<b>everything</b>"),
        "the description is escaped"
    );
    html(&form).assert_text("<script>alert(1)</script>");
    let list = send(&admin, "GET", "/admin/flags", "");
    assert!(!list.html().unwrap().contains("<b>everything</b>"));
}

#[test]
fn a_reader_who_may_not_act_sees_a_disabled_form_and_a_post_is_refused() {
    let flags = flags();
    let reader = policy_fn(|_: &Principal<'_>, r: Request<'_>| r.action != Action::ActOnPanel);
    let admin = admin_with(&flags, reader);
    let form = html(&send(&admin, "GET", "/admin/flags/search", ""));
    form.assert_no_button("Save");
    assert!(form.field("percent").unwrap().attr("disabled").is_some());
    let post = send(&admin, "POST", "/admin/flags/search", "on=1");
    assert_eq!(post.status(), 403);
    assert_eq!(flags.rule("search"), None);
}

#[test]
fn the_tab_is_locked_like_every_tab() {
    let flags = flags();
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .at("/admin")
            .panel(FlagsPanel::new(flags.clone(), ALL)),
    );
    assert_eq!(send(&admin, "GET", "/admin/flags", "").status(), 403);
    assert_eq!(
        send(&admin, "POST", "/admin/flags/search", "on=1").status(),
        403
    );
    assert_eq!(flags.rule("search"), None);
}

#[test]
fn a_failed_read_of_the_table_is_shown_above_the_last_rules() {
    struct Down;
    #[async_trait::async_trait]
    impl brocanth_flags::FlagStore for Down {
        async fn load(
            &self,
        ) -> Result<std::collections::BTreeMap<String, Rule>, brocanth_flags::StoreError> {
            Err(brocanth_flags::StoreError::new("connection <refused>"))
        }
        async fn save(&self, _: &str, _: &Rule) -> Result<(), brocanth_flags::StoreError> {
            Ok(())
        }
        async fn remove(&self, _: &str) -> Result<(), brocanth_flags::StoreError> {
            Ok(())
        }
    }
    let flags = Flags::with_store(Arc::new(Down));
    block_on(flags.set("search", Rule::everyone())).unwrap();
    let _ = block_on(flags.refresh());
    let page = html(&send(&admin(&flags), "GET", "/admin/flags", ""));
    page.assert_text("The last read of the table failed: connection <refused>")
        .assert_text("On for everyone");
}
