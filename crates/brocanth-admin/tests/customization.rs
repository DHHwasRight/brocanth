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

// ------------------------------------------------------------------ scripts

/// The opening `<script ...>` tags, read from the source: the HTML reader
/// skips raw-text elements.
fn script_tags(page: &Html) -> Vec<String> {
    page.source()
        .split("<script")
        .skip(1)
        .map(|rest| rest[..rest.find('>').unwrap_or(rest.len())].to_string())
        .collect()
}

fn scripts(page: &Html) -> Vec<String> {
    script_tags(page)
        .iter()
        .filter_map(|tag| {
            let at = tag.find("src=\"")? + 5;
            let len = tag[at..].find('"')?;
            Some(tag[at..at + len].to_string())
        })
        .collect()
}

#[test]
fn the_admin_script_is_on_every_screen_and_a_model_one_only_on_its_own() {
    let admin = blog(
        AdminConfig::new()
            .script("/assets/admin.js")
            .model("post", |m| m.script("/assets/posts.js")),
    );

    assert_eq!(scripts(&index(&admin)), ["/assets/admin.js"]);
    // The admin's first, then the model's, as the stylesheets.
    assert_eq!(
        scripts(&list(&admin, "post")),
        ["/assets/admin.js", "/assets/posts.js"]
    );
    assert_eq!(scripts(&list(&admin, "user")), ["/assets/admin.js"]);
    let detail = Html::new(
        block_on(admin.detail(Principal::anonymous(), "post", "1"))
            .unwrap()
            .html()
            .unwrap(),
    );
    assert!(scripts(&detail).contains(&"/assets/posts.js".to_string()));
}

#[test]
fn a_model_script_is_a_deferred_file_and_never_inline_code() {
    // The CSP decision: a file passes `script-src 'self'` with no hash. So
    // a model script adds no inline script, and the two inline scripts the
    // layout has are the ones a policy already allows by hash.
    let plain = list(&blog(AdminConfig::new()), "post");
    let with = list(
        &blog(AdminConfig::new().model("post", |m| m.script("/assets/posts.js"))),
        "post",
    );
    let inline = |page: &Html| {
        script_tags(page)
            .iter()
            .filter(|tag| !tag.contains("src="))
            .count()
    };
    assert_eq!(inline(&with), inline(&plain));
    assert_eq!(inline(&plain), 2, "dp-swap's two");
    let tags = script_tags(&with);
    let tag = tags
        .iter()
        .find(|tag| tag.contains("src=\"/assets/posts.js\""))
        .expect("the model script");
    assert!(tag.contains("defer"), "it waits for the document");
}

#[test]
fn a_swap_asks_for_the_page_itself_and_brings_its_head() {
    // dp-swap in header mode asks for the page's own URL, not
    // `<url>/_fragment.html`, which the admin does not serve: every
    // boosted link was a 404 and a full load. The line is in the one
    // inline script a Content-Security-Policy allows by hash.
    let script = brocanth_admin::INLINE_SCRIPT;
    assert!(
        script.ends_with("dpswap.configure({fragmentMode:\"header\"});\n"),
        "header mode"
    );
    // The vendored build merges the response's `<head>`: it adds the files
    // the live page lacks, which it finds by these selectors.
    assert!(script.contains("script[src]"), "the head merge is built in");
    assert!(script.contains(r#"link[rel~="stylesheet"][href]"#));

    // What a swap into the post list gets: the whole document, whose head
    // names the model's files. dp-swap adds them to the live head.
    let admin = blog(AdminConfig::new().model("post", |m| {
        m.script("/assets/posts.js").stylesheet("/p.css")
    }));
    let page = list(&admin, "post");
    assert!(page.source().contains("<head>"));
    let head = &page.source()[..page.source().find("</head>").expect("a head")];
    assert!(
        head.contains(r#"<script src="/assets/posts.js" defer>"#),
        "{head}"
    );
    assert!(
        head.contains(r#"<link rel="stylesheet" href="/p.css""#),
        "{head}"
    );
}

// ------------------------------------------------------------------- blocks

#[test]
fn a_brand_block_replaces_the_header_and_keeps_the_layout() {
    use brocanth_admin::view::Chrome;

    let admin = blog(AdminConfig::new().map_views(|views| {
        views.with_brand(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
            w.raw("<a href=\"/admin\"><img src=\"/logo.svg\" alt=\"Acme\"> ")?;
            w.text(&ctx.header)?;
            w.raw("</a>")
        })
    }));
    let page = list(&admin, "post");
    let home = page.link_to("/admin").expect("the brand links home");
    assert!(home.text().contains("Blog admin"));
    assert_eq!(page.tags("img").len(), 1, "the logo is in");
    // The rest of the layout is the shipped one: the nav and the screen.
    assert_eq!(page.tags("nav").len(), 1);
    assert_eq!(page.tags("table").len(), 1);
}

struct User {
    email: &'static str,
}

fn named(who: &Principal<'_>) -> Option<String> {
    who.downcast::<User>().map(|u| u.email.to_string())
}

fn index_as(admin: &Admin, user: &User) -> Html {
    Html::new(
        block_on(admin.index(Principal::new(user)))
            .unwrap()
            .html()
            .expect("a page"),
    )
}

#[test]
fn the_user_links_name_who_is_signed_in() {
    let ada = User {
        email: "ada@example.com",
    };
    let admin = blog(AdminConfig::new().user_label(named));
    assert!(index_as(&admin, &ada)
        .text()
        .contains("Signed in as ada@example.com"));
    // Nobody signed in: no name.
    assert!(!index(&admin).text().contains("Signed in"));
}

#[test]
fn a_user_links_block_draws_what_the_application_has_and_keeps_the_layout() {
    use brocanth_admin::view::Chrome;

    let ada = User {
        email: "ada@example.com",
    };
    // Django's `userlinks` block: the application draws "Log out".
    let admin = blog(AdminConfig::new().user_label(named).map_views(|views| {
        views.with_userlinks(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
            if let Some(user) = &ctx.user {
                w.text(user)?;
            }
            w.raw(" <a href=\"/logout\">Log out</a>")
        })
    }));
    let page = index_as(&admin, &ada);
    assert_eq!(page.link_to("/logout").unwrap().text().trim(), "Log out");
    assert!(page.text().contains("ada@example.com"));
    assert!(
        !page.text().contains("Signed in as"),
        "the shipped block is gone"
    );
    assert_eq!(page.tags("nav").len(), 1, "the layout is the shipped one");
}

#[test]
fn a_replaced_layout_receives_the_blocks_rendered() {
    use brocanth_admin::view::{Chrome, LayoutCtx};

    let admin = blog(AdminConfig::new().map_views(|views| {
        views
            .with_brand(|_: &Chrome, w: &mut rsx::Writer<'_>| w.raw("<b>Brand</b>"))
            .with_layout(|ctx: &LayoutCtx, w: &mut rsx::Writer<'_>| {
                w.raw("<header>")?;
                w.raw(&ctx.brand)?;
                w.raw("</header><main>")?;
                w.raw(&ctx.body)?;
                w.raw("</main>")
            })
    }));
    assert_eq!(texts(&index(&admin), "header"), ["Brand"]);
}
