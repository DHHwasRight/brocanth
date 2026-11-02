//! The levels of `docs/admin-customization.md`, each asserted on what a
//! browser receives: the titles, the stylesheets, the "View site" link, a
//! screen replaced for one model, and a shipped screen wrapped.

use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::view::ListCtx;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal, Views};
use brocanth_test::Html;
use rsx::Template;

mod views {
    rsx::load_dir!("tests/views");
}

fn blog(config: AdminConfig) -> Admin {
    Admin::new(
        Arc::new(Fixture::blog()),
        config
            .titled("Blog admin")
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment),
    )
}

fn index(admin: &Admin) -> Html {
    Html::new(
        block_on(admin.index(Principal::anonymous()))
            .unwrap()
            .html()
            .expect("a page"),
    )
}

fn list(admin: &Admin, model: &str) -> Html {
    Html::new(
        block_on(admin.list(Principal::anonymous(), model, ListQuery::new()))
            .unwrap()
            .html()
            .expect("a page"),
    )
}

fn stylesheets(page: &Html) -> Vec<String> {
    page.tags("link")
        .iter()
        .filter(|l| l.attr("rel") == Some("stylesheet"))
        .filter_map(|l| l.attr("href").map(str::to_string))
        .collect()
}

fn texts(page: &Html, tag: &str) -> Vec<String> {
    page.tags(tag)
        .iter()
        .map(|e| e.text().trim().to_string())
        .collect()
}

// ---------------------------------------------------------------- settings

#[test]
fn the_title_names_the_header_and_the_index_until_they_are_named_apart() {
    let page = index(&blog(AdminConfig::new()));
    assert_eq!(texts(&page, "title"), ["Blog admin"]);
    assert_eq!(texts(&page, "h1"), ["Blog admin"]);
    assert!(page.has_link_to("/admin"), "the header links home");
    assert_eq!(page.link_to("/admin").unwrap().text().trim(), "Blog admin");

    let page = index(&blog(
        AdminConfig::new()
            .headed("Blog HQ")
            .index_titled("Site administration"),
    ));
    // Django's three: `site_title` at the end of the tab, `site_header` at
    // the top of the sidebar, `index_title` over the models and first in
    // the tab.
    assert_eq!(texts(&page, "title"), ["Site administration · Blog admin"]);
    assert_eq!(page.link_to("/admin").unwrap().text().trim(), "Blog HQ");
    assert_eq!(texts(&page, "h1"), ["Site administration"]);
}

#[test]
fn a_site_url_draws_a_view_site_link_and_none_draws_nothing() {
    let page = index(&blog(AdminConfig::new()));
    assert!(!page.text().contains("View site"));

    let page = index(&blog(AdminConfig::new().site_url("/")));
    assert_eq!(page.link_to("/").unwrap().text().trim(), "View site");
}

// ------------------------------------------------------------- stylesheets

#[test]
fn the_admin_stylesheet_is_on_every_screen_and_a_model_one_only_on_its_own() {
    let admin = blog(
        AdminConfig::new()
            .stylesheet("/assets/admin-theme.css")
            .model("post", |m| m.stylesheet("/assets/posts.css")),
    );

    assert_eq!(stylesheets(&index(&admin)), ["/assets/admin-theme.css"]);
    // The admin's first, then the model's, so the model's rule wins a tie.
    assert_eq!(
        stylesheets(&list(&admin, "post")),
        ["/assets/admin-theme.css", "/assets/posts.css"]
    );
    assert_eq!(
        stylesheets(&list(&admin, "user")),
        ["/assets/admin-theme.css"]
    );

    let detail = Html::new(
        block_on(admin.detail(Principal::anonymous(), "post", "1"))
            .unwrap()
            .html()
            .unwrap(),
    );
    assert!(stylesheets(&detail).contains(&"/assets/posts.css".to_string()));
}

#[test]
fn the_shipped_stylesheet_stays_inline_when_a_theme_is_added() {
    // A theme is a few custom properties over the shipped sheet, not a
    // replacement for it: the `<style>` must still be there, before the
    // link. The reader skips raw-text elements, so this reads the source.
    let page = index(&blog(AdminConfig::new().stylesheet("/theme.css")));
    let source = page.source();
    let style = source.find("<style>").expect("the shipped sheet");
    let link = source.find("href=\"/theme.css\"").expect("the theme");
    assert!(style < link, "the theme comes after the shipped sheet");
    assert!(source[style..link].contains("--color-accent"));
}

// ------------------------------------------------------- per-model screens

#[test]
fn a_model_screen_wins_over_the_admin_screen_and_only_for_that_model() {
    let admin = blog(
        AdminConfig::new()
            .with_views(
                Views::default()
                    .with_list(|_: &ListCtx, w: &mut rsx::Writer<'_>| w.raw("<p>every list</p>")),
            )
            .model("post", |m| {
                m.list_view(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
                    views::custom::PostList { ctx }.render(w)
                })
            }),
    );

    let posts = list(&admin, "post");
    assert_eq!(texts(&posts, "h1"), ["Our posts"]);
    assert!(!posts.text().contains("every list"));

    let users = list(&admin, "user");
    assert!(users.text().contains("every list"), "the admin's override");
    assert!(users.tags("h1").is_empty());
}

#[test]
fn an_override_can_wrap_the_shipped_screen() {
    // Django's `{{ block.super }}`: keep the default, add to it.
    let shipped = Views::default().list;
    let admin = blog(AdminConfig::new().model("post", |m| {
        m.list_view(move |ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
            w.raw("<p role=\"note\">Edits here reach customers.</p>")?;
            shipped.render(ctx, w)
        })
    }));

    let page = list(&admin, "post");
    assert!(page.text().contains("Edits here reach customers."));
    assert_eq!(
        page.tags("table").len(),
        1,
        "and the shipped table is there"
    );
    assert!(page.text().contains("On the Analytical Engine"));
}
