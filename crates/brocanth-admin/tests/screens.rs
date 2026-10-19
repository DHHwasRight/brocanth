//! The four screens end to end, and the seam that replaces any of them.

use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::view::{LayoutCtx, ListCtx};
use brocanth_admin::{
    Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal, Submission, Views,
};
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
            .with_policy(AllowAllUnsafeForDevelopment)
            .model("user", |m| {
                m.display(|row| row.get("name").map(ToString::to_string).unwrap_or_default())
            }),
    )
}

fn html(response: brocanth_admin::Response) -> String {
    response.html().expect("a page").to_string()
}

// ------------------------------------------------------------- the four screens

#[test]
fn the_list_paginates_searches_and_sorts() {
    let admin = blog(AdminConfig::new().model("post", |m| {
        m.list_columns(["id", "title", "views"])
            .searchable(["title"])
            .per_page(1)
    }));
    let who = Principal::anonymous();

    let first = html(block_on(admin.list(who, "post", ListQuery::new())).unwrap());
    assert!(first.contains("On the Analytical Engine"));
    assert!(!first.contains("Compilers"), "page one holds one row");
    assert!(first.contains("page 1 of 2"));

    let second = html(block_on(admin.list(who, "post", ListQuery::new().page(2))).unwrap());
    assert!(second.contains("Compilers"));

    let searched =
        html(block_on(admin.list(who, "post", ListQuery::new().searching("compil"))).unwrap());
    assert!(searched.contains("Compilers"));
    assert!(!searched.contains("On the Analytical Engine"));

    // A sort link exists for a listed column and names the opposite direction.
    assert!(first.contains("sort=views&amp;dir=asc"), "{first}");
}

#[test]
fn a_sort_naming_a_column_that_does_not_exist_is_dropped() {
    let admin = blog(AdminConfig::new());
    let query = ListQuery::parse("?sort=oops%27%3B+DROP+TABLE&dir=desc");
    let response = block_on(admin.list(Principal::anonymous(), "post", query)).unwrap();

    assert_eq!(
        response.status(),
        200,
        "a bad sort shows a list, not a failure"
    );
}

#[test]
fn the_detail_screen_shows_one_record() {
    let admin = blog(AdminConfig::new());
    let page = html(block_on(admin.detail(Principal::anonymous(), "user", "1")).unwrap());

    assert!(page.contains("Ada Lovelace"));
    assert!(page.contains("ada@example.com"));
    assert!(page.contains("/admin/user/1/edit"));
    assert!(page.contains("/admin/user/1/delete"));
}

#[test]
fn a_record_that_does_not_exist_is_a_404() {
    let admin = blog(AdminConfig::new());
    let response = block_on(admin.detail(Principal::anonymous(), "user", "999")).unwrap();
    assert_eq!(response.status(), 404);

    let response = block_on(admin.list(Principal::anonymous(), "nope", ListQuery::new())).unwrap();
    assert_eq!(response.status(), 404);
}

#[test]
fn the_form_creates_and_then_edits() {
    let admin = blog(AdminConfig::new());
    let who = Principal::anonymous();

    let form = html(block_on(admin.create_form(who, "user")).unwrap());
    assert!(form.contains("name=\"email\""));
    assert!(
        !form.contains("name=\"id\""),
        "a generated key is not asked for"
    );

    let created = block_on(
        admin.create(
            who,
            "user",
            &Submission::new()
                .with("name", "Barbara Liskov")
                .with("email", "barbara@example.com")
                .with("bio", "")
                .with("role", "author")
                .with("active", "on"),
        ),
    )
    .unwrap();
    assert_eq!(created.status(), 303);
    assert_eq!(created.location(), Some("/admin/user/3"));

    let edit = html(block_on(admin.edit_form(who, "user", "3")).unwrap());
    assert!(edit.contains("Barbara Liskov"));

    let updated = block_on(
        admin.update(
            who,
            "user",
            "3",
            &Submission::new()
                .with("name", "Barbara H. Liskov")
                .with("email", "barbara@example.com")
                .with("role", "author"),
        ),
    )
    .unwrap();
    assert_eq!(updated.status(), 303);

    let detail = html(block_on(admin.detail(who, "user", "3")).unwrap());
    assert!(detail.contains("Barbara H. Liskov"));
}

#[test]
fn deleting_asks_first() {
    let data = Arc::new(Fixture::blog());
    let admin = Admin::new(
        Arc::clone(&data) as Arc<dyn brocanth_admin::AdminData>,
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    let who = Principal::anonymous();

    let confirm = html(block_on(admin.delete_confirm(who, "post", "1")).unwrap());
    assert!(confirm.contains("cannot be undone"));
    assert!(confirm.contains("method=\"post\""), "a GET must not delete");
    assert_eq!(data.len("post"), 2, "the confirmation changed nothing");

    let deleted = block_on(admin.delete(who, "post", "1")).unwrap();
    assert_eq!(deleted.status(), 303);
    assert_eq!(deleted.location(), Some("/admin/post"));
    assert_eq!(data.len("post"), 1);
}

#[test]
fn routing_reaches_every_screen() {
    use brocanth_admin::Route;

    let base = "/admin";
    assert_eq!(Route::parse(base, "GET", "/admin", &[]), Some(Route::Index));
    assert_eq!(
        Route::parse(base, "GET", "/admin/", &[]),
        Some(Route::Index)
    );
    assert_eq!(
        Route::parse(base, "GET", "/admin/post", &[]),
        Some(Route::List("post"))
    );
    assert_eq!(
        Route::parse(base, "GET", "/admin/post/new", &[]),
        Some(Route::New("post"))
    );
    assert_eq!(
        Route::parse(base, "POST", "/admin/post", &[]),
        Some(Route::Create("post"))
    );
    assert_eq!(
        Route::parse(base, "GET", "/admin/post/7", &[]),
        Some(Route::Detail("post", "7"))
    );
    assert_eq!(
        Route::parse(base, "GET", "/admin/post/7/edit", &[]),
        Some(Route::Edit("post", "7"))
    );
    assert_eq!(
        Route::parse(base, "POST", "/admin/post/7", &[]),
        Some(Route::Update("post", "7"))
    );
    assert_eq!(
        Route::parse(base, "GET", "/admin/post/7/delete", &[]),
        Some(Route::ConfirmDelete("post", "7"))
    );
    assert_eq!(
        Route::parse(base, "POST", "/admin/post/7/delete", &[]),
        Some(Route::Delete("post", "7"))
    );
    assert_eq!(Route::parse(base, "DELETE", "/admin/post/7", &[]), None);
    assert_eq!(Route::parse(base, "GET", "/admin/a/b/c/d", &[]), None);
}

#[test]
fn dispatch_carries_the_query_string_through() {
    let admin = blog(AdminConfig::new().model("post", |m| m.per_page(1)));
    let response = block_on(admin.dispatch(
        Principal::anonymous(),
        "GET",
        "/admin/post",
        "page=2",
        &Submission::new(),
    ))
    .unwrap();

    assert!(html(response).contains("Compilers"));
}

// ---------------------------------------------------- foreign keys show labels

#[test]
fn a_foreign_key_renders_a_label_rather_than_an_id() {
    let admin = blog(AdminConfig::new().model("post", |m| {
        m.list_columns(["title", "author_id"]).display(|row| {
            row.get("title")
                .map(ToString::to_string)
                .unwrap_or_default()
        })
    }));
    let page =
        html(block_on(admin.list(Principal::anonymous(), "post", ListQuery::new())).unwrap());

    assert!(
        page.contains("Ada Lovelace"),
        "the author's name, not their id"
    );
    assert!(page.contains("Grace Hopper"));
    assert!(page.contains("/admin/user/1"), "and it links to the record");
    // The header drops the `_id`, because the cell holds a person.
    assert!(page.contains(">Author<"), "{page}");
}

#[test]
fn a_foreign_key_picker_offers_names() {
    let admin = blog(AdminConfig::new());
    let form = html(block_on(admin.create_form(Principal::anonymous(), "post")).unwrap());

    assert!(form.contains("<select"), "not a number input");
    assert!(form.contains("value=\"1\">Ada Lovelace</option>"), "{form}");
    assert!(form.contains("value=\"2\">Grace Hopper</option>"));
}

#[test]
fn a_foreign_key_shows_a_label_on_the_detail_screen_too() {
    let admin = blog(AdminConfig::new());
    let page = html(block_on(admin.detail(Principal::anonymous(), "post", "1")).unwrap());

    assert!(page.contains("Ada Lovelace"));
}

// --------------------------------------------------------------- the seam

#[test]
fn an_overridden_rsx_component_is_used_in_place_of_the_default() {
    let shipped = html(
        block_on(blog(AdminConfig::new()).list(Principal::anonymous(), "post", ListQuery::new()))
            .unwrap(),
    );
    assert!(shipped.contains("<table"), "the shipped list is a table");
    assert!(!shipped.contains("my-own-list"));

    let admin = blog(AdminConfig::new().with_views(Views::default().with_list(
        |ctx: &ListCtx, w: &mut rsx::Writer<'_>| views::custom::PostList { ctx }.render(w),
    )));
    let overridden =
        html(block_on(admin.list(Principal::anonymous(), "post", ListQuery::new())).unwrap());

    assert!(overridden.contains("my-own-list"), "{overridden}");
    assert!(overridden.contains("<h1>Our posts</h1>"));
    assert!(
        overridden.contains("On the Analytical Engine"),
        "it got real rows"
    );
    assert!(!overridden.contains("<table"), "the default did not run");

    // One screen was replaced; the rest are still the shipped ones.
    assert!(
        overridden.contains("rail"),
        "the shipped layout still wraps it"
    );
    let detail = html(block_on(admin.detail(Principal::anonymous(), "post", "1")).unwrap());
    assert!(detail.contains("<table"), "the detail screen is untouched");
}

#[test]
fn the_layout_is_a_seam_as_well() {
    let admin = blog(AdminConfig::new().with_views(Views::default().with_layout(
        |ctx: &LayoutCtx, w: &mut rsx::Writer<'_>| views::custom::Shell { ctx }.render(w),
    )));
    let page = html(block_on(admin.index(Principal::anonymous())).unwrap());

    assert!(page.contains("class=\"our-shell\""));
    assert!(
        !page.contains("class=\"rail\""),
        "the shipped shell did not run"
    );
    assert!(
        page.contains("Posts"),
        "and the index screen still rendered"
    );
}

#[test]
fn a_view_may_be_any_value_that_implements_the_trait() {
    // Not only a closure: a struct with state works, which is what a view
    // needing configuration of its own would be.
    struct Banner {
        text: String,
    }

    impl brocanth_admin::View<ListCtx> for Banner {
        fn render(&self, ctx: &ListCtx, w: &mut rsx::Writer<'_>) -> rsx::Result {
            w.raw("<p id=\"banner\">")?;
            w.text(&self.text)?;
            w.raw("</p><p>")?;
            w.text(ctx.rows.len())?;
            w.raw(" rows</p>")
        }
    }

    let admin = blog(
        AdminConfig::new().with_views(Views::default().with_list(Banner {
            text: "Mind the step".to_string(),
        })),
    );
    let page =
        html(block_on(admin.list(Principal::anonymous(), "post", ListQuery::new())).unwrap());

    assert!(page.contains("id=\"banner\""));
    assert!(page.contains("2 rows"));
}

// ------------------------------------------------------------------ escaping

#[test]
fn a_value_cannot_write_markup() {
    let data = Fixture::blog().row(
        "post",
        brocanth_admin::Row::new()
            .with("id", 9)
            .with("title", "<script>alert(1)</script>")
            .with("body", "x")
            .with("author_id", 1)
            .with("published", false)
            .with("views", 0)
            .with("published_at", brocanth_admin::Value::Null),
    );
    let admin = Admin::new(
        Arc::new(data),
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment),
    );
    let page =
        html(block_on(admin.list(Principal::anonymous(), "post", ListQuery::new())).unwrap());

    assert!(!page.contains("<script>alert(1)</script>"));
    assert!(page.contains("&lt;script&gt;"));
}
