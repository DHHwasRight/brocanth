//! An ejected screen renders what the shipped one does.
//!
//! `tests/ejected/` holds the copies `brocanth admin eject` writes (a unit
//! test in `src/eject.rs` keeps them current). This file compiles them the
//! way an application does: as an outside crate, naming `brocanth_admin::`
//! and nothing private. Then it installs them and compares every screen's
//! HTML with the default's, byte for byte, on the pages and branches the
//! blog fixture reaches.

use std::sync::Arc;

use brocanth_admin::actions::BulkAction;
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::view::{
    ActionCtx, Chrome, DeleteCtx, DetailCtx, ErrorCtx, FormCtx, IndexCtx, LayoutCtx, ListCtx,
};
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission, Views};
use rsx::{Template as _, Writer};

mod ejected {
    rsx::load_dir!("tests/ejected");
}

/// Every screen replaced by its ejected copy.
fn ejected_views() -> Views {
    Views::default()
        .with_layout(|ctx: &LayoutCtx, w: &mut Writer<'_>| {
            ejected::layout::Layout { ctx }.render(w)
        })
        .with_index(|ctx: &IndexCtx, w: &mut Writer<'_>| ejected::index::Index { ctx }.render(w))
        .with_confirm_action(|ctx: &ActionCtx, w: &mut Writer<'_>| {
            ejected::action::ConfirmAction { ctx }.render(w)
        })
        .with_list(|ctx: &ListCtx, w: &mut Writer<'_>| ejected::list::List { ctx }.render(w))
        .with_detail(|ctx: &DetailCtx, w: &mut Writer<'_>| {
            ejected::detail::Detail { ctx }.render(w)
        })
        .with_form(|ctx: &FormCtx, w: &mut Writer<'_>| ejected::form::FormPage { ctx }.render(w))
        .with_confirm_delete(|ctx: &DeleteCtx, w: &mut Writer<'_>| {
            ejected::delete::Delete { ctx }.render(w)
        })
        .with_error(|ctx: &ErrorCtx, w: &mut Writer<'_>| {
            ejected::error::ErrorPage { ctx }.render(w)
        })
        .with_brand(|ctx: &Chrome, w: &mut Writer<'_>| ejected::brand::Brand { ctx }.render(w))
        .with_userlinks(|ctx: &Chrome, w: &mut Writer<'_>| {
            ejected::userlinks::Userlinks { ctx }.render(w)
        })
}

fn blog(config: AdminConfig) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        config
            .titled("Blog admin")
            .at("/admin")
            .site_url("/")
            .stylesheet("/theme.css")
            .script("/theme.js")
            .user_label(|_| Some("Ada".to_string()))
            // The checkbox column, the action menu and an extra record link
            // are part of the screens, so the comparison covers them.
            .action(BulkAction::delete_selected())
            .record_link("History", |r| {
                Some(format!("/history/{}/{}", r.table, r.key))
            })
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment),
    )
}

fn page(admin: &Admin, method: &str, path: &str, query: &str, form: &Submission) -> String {
    let answer = block_on(admin.dispatch(Principal::anonymous(), method, path, query, form))
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"));
    answer
        .html()
        .unwrap_or_else(|| panic!("{method} {path} answered with no page"))
        .to_string()
}

/// The requests that reach every screen and their main branches: an empty
/// and a searched list, a sorted one, a foreign key, a create and an edit
/// form, a form with errors, a delete confirmation, a 404.
const GETS: &[(&str, &str)] = &[
    ("/admin", ""),
    ("/admin/post", ""),
    ("/admin/post", "sort=title&dir=desc"),
    ("/admin/post", "q=nothing-matches-this"),
    ("/admin/user", "q=a"),
    ("/admin/post/1", ""),
    ("/admin/user/1", ""),
    ("/admin/post/new", ""),
    ("/admin/post/1/edit", ""),
    ("/admin/user/1/edit", ""),
    ("/admin/post/1/delete", ""),
    ("/admin/post/999", ""),
    ("/admin/nothing-here", ""),
];

#[test]
fn every_ejected_screen_renders_what_the_shipped_one_does() {
    let shipped = blog(AdminConfig::new());
    let ejected = blog(AdminConfig::new().with_views(ejected_views()));
    let nothing = Submission::new();
    for (path, query) in GETS {
        let want = page(&shipped, "GET", path, query, &nothing);
        let got = page(&ejected, "GET", path, query, &nothing);
        assert!(
            want == got,
            "GET {path}?{query} differs once ejected\n--- shipped\n{want}\n--- ejected\n{got}"
        );
    }
}

#[test]
fn an_ejected_action_confirmation_renders_what_the_shipped_one_does() {
    let shipped = blog(AdminConfig::new());
    let ejected = blog(AdminConfig::new().with_views(ejected_views()));
    let chosen = Submission::from_urlencoded("action=delete_selected&pk=1&pk=2&pk=999");
    let want = page(&shipped, "POST", "/admin/post/actions", "", &chosen);
    let got = page(&ejected, "POST", "/admin/post/actions", "", &chosen);
    assert!(want.contains("confirm"), "no confirmation was drawn");
    assert_eq!(want, got);
}

#[test]
fn an_ejected_form_shows_errors_as_the_shipped_one_does() {
    let shipped = blog(AdminConfig::new());
    let ejected = blog(AdminConfig::new().with_views(ejected_views()));
    // A title over its maximum length and a missing body: refused, so the
    // form comes back with its errors and the values that were sent.
    let refused =
        Submission::from_urlencoded(&format!("title={}&author_id=1&views=x", "t".repeat(200)));
    let want = page(&shipped, "POST", "/admin/post", "", &refused);
    let got = page(&ejected, "POST", "/admin/post", "", &refused);
    assert!(
        want.contains("aria-invalid") || want.contains("error"),
        "the fixture refused nothing"
    );
    assert_eq!(want, got);
}

#[test]
fn an_ejected_screen_for_one_model_leaves_the_others_shipped() {
    let shipped = blog(AdminConfig::new());
    let one = blog(AdminConfig::new().model("post", |m| {
        m.list_view(|ctx: &ListCtx, w: &mut Writer<'_>| ejected::list::List { ctx }.render(w))
    }));
    let nothing = Submission::new();
    for path in ["/admin/post", "/admin/user"] {
        assert_eq!(
            page(&shipped, "GET", path, "", &nothing),
            page(&one, "GET", path, "", &nothing)
        );
    }
}
