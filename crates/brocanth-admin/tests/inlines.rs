//! Inlines: a user's posts on the user's form. Django's `InlineModelAdmin`.
//!
//! Against the in-memory fixture, so every rule that does not need SQL is
//! here. The row rules on a real database are in
//! `crates/brocanth-admin-model/tests/inlines.rs`.

use std::sync::Arc;

use brocanth_admin::data::{AdminData, ListRequest, Scope, Search};
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Policy, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, Discovery, Fixture, Inline, PrimaryKey, Principal, Response, Row,
    Submission, Value,
};
use brocanth_test::Html;

/// The blog, with a second post by Ada, so her form has two children.
fn blog() -> Arc<Fixture> {
    Arc::new(
        Fixture::blog().row(
            "post",
            Row::new()
                .with("id", 3)
                .with("title", "Sketch of the Engine")
                .with("body", "Notes.")
                .with("author_id", 1)
                .with("published", false)
                .with("views", 3)
                .with("published_at", Value::Null),
        ),
    )
}

fn admin_with(data: Arc<Fixture>, policy: impl Policy, inline: Inline) -> Admin {
    Admin::new(
        data,
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy)
            .model("user", |m| m.inline(inline)),
    )
}

fn admin(data: Arc<Fixture>) -> Admin {
    admin_with(
        data,
        AllowAllUnsafeForDevelopment,
        Inline::new("post").fields(["title", "views", "published"]),
    )
}

fn get(admin: &Admin, path: &str) -> Response {
    block_on(admin.dispatch(Principal::anonymous(), "GET", path, "", &Submission::new())).unwrap()
}

fn post(admin: &Admin, path: &str, form: Submission) -> Response {
    block_on(admin.dispatch(Principal::anonymous(), "POST", path, "", &form)).unwrap()
}

fn html(response: &Response) -> Html {
    Html::new(response.html().expect("a page"))
}

/// Ada's own fields, as her edit form posts them.
fn ada() -> Submission {
    Submission::new()
        .with("name", "Ada Lovelace")
        .with("email", "ada@example.com")
        .with("bio", "Wrote the first program.")
        .with("role", "editor")
        .with("active", "on")
        .with("joined_at", "1843-10-01T00:00")
}

/// One post as the data source holds it.
fn stored(data: &Fixture, admin: &Admin, id: i64) -> Option<Row> {
    let model = admin.descriptor("post").expect("the post model");
    block_on(data.fetch(model, &PrimaryKey::single("id", id), Scope::Unscoped)).unwrap()
}

fn title(data: &Fixture, admin: &Admin, id: i64) -> Option<String> {
    stored(data, admin, id).map(|row| row.get("title").map(Value::as_input).unwrap_or_default())
}

/// Every post, as `(author_id, title)`.
fn all_posts(data: &Fixture, admin: &Admin) -> Vec<(i64, String)> {
    let model = admin.descriptor("post").expect("the post model");
    let rows = block_on(data.list(ListRequest {
        model,
        offset: 0,
        limit: 100,
        search: Search {
            term: "",
            columns: &[],
        },
        sort: None,
        filter: None,
        scope: Scope::Unscoped,
    }))
    .unwrap();
    rows.iter()
        .map(|row| {
            let author = match row.get("author_id") {
                Some(Value::Int(n)) => *n,
                _ => -1,
            };
            (
                author,
                row.get("title").map(Value::as_input).unwrap_or_default(),
            )
        })
        .collect()
}

#[test]
fn the_edit_form_holds_the_children_in_the_same_form_with_a_blank_row() {
    let admin = admin(blog());
    let page = html(&get(&admin, "/admin/user/1/edit"));

    // One form, and the child rows are in it: one POST, one CSRF token.
    assert_eq!(page.forms().len(), 1);
    let form = page.form_posting_to("/admin/user/1").expect("the form");
    let names: Vec<String> = form
        .html()
        .fields()
        .iter()
        .filter_map(|f| f.attr("name").map(str::to_string))
        .collect();
    for name in [
        "name",
        "post-0-pk",
        "post-0-title",
        "post-0-views",
        "post-0-published",
        "post-0-delete",
        "post-1-pk",
        "post-1-title",
        "post-2-title",
    ] {
        assert!(names.iter().any(|n| n == name), "{name} in {names:?}");
    }
    // The blank row has no key and no delete box; the column that points
    // at the parent is never a field.
    assert!(!names
        .iter()
        .any(|n| n == "post-2-pk" || n == "post-2-delete"));
    assert!(!names.iter().any(|n| n.ends_with("-author_id")));
    // Ada's two posts, in key order, and not Grace's.
    assert_eq!(page.field("post-0-pk").unwrap().attr("value"), Some("1"));
    assert_eq!(page.field("post-1-pk").unwrap().attr("value"), Some("3"));
    page.assert_no_text("Compilers");
    page.assert_text("Sketch of the Engine");
    page.assert_link_to("/admin/post/3");
}

#[test]
fn one_post_changes_adds_and_deletes_children() {
    let data = blog();
    let admin = admin(data.clone());
    let form = ada()
        .with("post-0-pk", "1")
        .with("post-0-title", "On the Analytical Engine, revised")
        .with("post-0-views", "1843")
        .with("post-0-published", "on")
        .with("post-1-pk", "3")
        .with("post-1-title", "Sketch of the Engine")
        .with("post-1-views", "3")
        .with("post-1-delete", "on")
        .with("post-2-title", "Note G")
        .with("post-2-views", "0");

    let response = post(&admin, "/admin/user/1", form);
    assert_eq!(response.location(), Some("/admin/user/1"), "{response:?}");

    assert_eq!(
        title(&data, &admin, 1).as_deref(),
        Some("On the Analytical Engine, revised")
    );
    let posts = all_posts(&data, &admin);
    assert!(
        !posts
            .iter()
            .any(|(_, title)| title == "Sketch of the Engine"),
        "deleted: {posts:?}"
    );
    assert!(
        posts.contains(&(1, "Note G".to_string())),
        "added, pointing at the parent: {posts:?}"
    );
    // Grace's post is not Ada's and was not touched.
    assert_eq!(title(&data, &admin, 2).as_deref(), Some("Compilers"));
}

#[test]
fn a_blank_row_adds_nothing() {
    let data = blog();
    let admin = admin(data.clone());
    let response = post(
        &admin,
        "/admin/user/1",
        ada().with("post-2-title", "").with("post-2-views", ""),
    );
    assert_eq!(response.status(), 303);
    assert_eq!(data.len("post"), 3);
}

#[test]
fn a_bad_child_row_is_shown_beside_its_row_and_nothing_is_written() {
    let data = blog();
    let admin = admin(data.clone());
    let long = "x".repeat(121);
    let form = ada()
        .with("name", "Ada, Countess of Lovelace")
        .with("post-0-pk", "1")
        .with("post-0-title", "Kept as typed")
        .with("post-0-views", "1843")
        .with("post-1-pk", "3")
        .with("post-1-title", &long)
        .with("post-1-views", "3")
        .with("post-2-title", "Note G")
        .with("post-2-views", "lots");

    let response = post(&admin, "/admin/user/1", form);
    assert_eq!(response.status(), 422);
    let page = html(&response);
    page.assert_text("must be 120 characters or fewer")
        .assert_text("must be a number");
    // What was typed survives, in every row.
    assert_eq!(
        page.field("post-0-title").unwrap().attr("value"),
        Some("Kept as typed")
    );
    assert_eq!(
        page.field("post-2-title").unwrap().attr("value"),
        Some("Note G")
    );
    assert_eq!(
        page.field("name").unwrap().attr("value"),
        Some("Ada, Countess of Lovelace")
    );

    // Nothing was written: not the parent, not the valid row, not the new one.
    assert_eq!(
        title(&data, &admin, 1).as_deref(),
        Some("On the Analytical Engine")
    );
    assert_eq!(data.len("post"), 3);
    let user = admin.descriptor("user").unwrap();
    let ada = block_on(data.fetch(user, &PrimaryKey::single("id", 1), Scope::Unscoped))
        .unwrap()
        .unwrap();
    assert_eq!(ada.get("name"), Some(&Value::Text("Ada Lovelace".into())));
}

/// Changing a post is allowed on post 1 and refused on post 3; deleting is
/// refused on every post.
fn per_row() -> impl Policy {
    policy_fn(|_: &Principal<'_>, request: Request<'_>| {
        let three = PrimaryKey::single("id", 3);
        match (request.action, request.model, request.record) {
            (Action::Update, "post", Some(key)) => *key != three,
            (Action::Delete, "post", _) => false,
            _ => true,
        }
    })
}

#[test]
fn the_policy_is_asked_per_child_row() {
    let data = blog();
    let admin = admin_with(
        data.clone(),
        per_row(),
        Inline::new("post").fields(["title", "views"]),
    );
    let page = html(&get(&admin, "/admin/user/1/edit"));
    // Post 1 is editable; post 3 is shown read-only and posts nothing.
    page.assert_field("post-0-title");
    assert!(page.field("post-1-title").is_none());
    assert!(page.field("post-1-pk").is_none());
    page.assert_text("Sketch of the Engine");
    // No row may be deleted.
    assert!(page.field("post-0-delete").is_none());

    // A POST made without the form, changing post 3: refused, and nothing
    // is written, not even the change to post 1 that was allowed.
    let crafted = ada()
        .with("post-0-pk", "1")
        .with("post-0-title", "Allowed")
        .with("post-0-views", "1")
        .with("post-1-pk", "3")
        .with("post-1-title", "Not allowed")
        .with("post-1-views", "3");
    assert_eq!(post(&admin, "/admin/user/1", crafted).status(), 403);
    assert_eq!(
        title(&data, &admin, 1).as_deref(),
        Some("On the Analytical Engine")
    );
    assert_eq!(
        title(&data, &admin, 3).as_deref(),
        Some("Sketch of the Engine")
    );

    // A delete the policy refuses: 403, and the post stays.
    let delete = ada().with("post-0-pk", "1").with("post-0-delete", "on");
    assert_eq!(post(&admin, "/admin/user/1", delete).status(), 403);
    assert!(stored(&data, &admin, 1).is_some());

    // The allowed change alone goes through.
    let allowed = ada()
        .with("post-0-pk", "1")
        .with("post-0-title", "Allowed")
        .with("post-0-views", "1");
    assert_eq!(post(&admin, "/admin/user/1", allowed).status(), 303);
    assert_eq!(title(&data, &admin, 1).as_deref(), Some("Allowed"));
}

#[test]
fn a_child_of_another_parent_cannot_be_changed_through_this_one() {
    let data = blog();
    let admin = admin(data.clone());
    // Post 2 is Grace's. Its key on Ada's form is not one of Ada's rows.
    let crafted = ada()
        .with("post-0-pk", "2")
        .with("post-0-title", "Taken over")
        .with("post-0-views", "12");
    assert_eq!(post(&admin, "/admin/user/1", crafted).status(), 409);
    assert_eq!(title(&data, &admin, 2).as_deref(), Some("Compilers"));
}

#[test]
fn no_create_on_the_child_draws_no_blank_row_and_refuses_one() {
    let data = blog();
    let admin = Admin::new(
        data.clone(),
        AdminConfig::new()
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
                !(request.action == Action::Create && request.model == "post")
            }))
            .model("user", |m| m.inline(Inline::new("post").fields(["title"]))),
    );
    let page = html(&get(&admin, "/admin/user/1/edit"));
    assert!(page.field("post-2-title").is_none(), "no blank row");
    let crafted = ada().with("post-2-title", "Sneaked in");
    assert_eq!(post(&admin, "/admin/user/1", crafted).status(), 403);
    assert_eq!(data.len("post"), 3);
}

#[test]
fn the_create_form_adds_the_parent_and_its_children() {
    let data = blog();
    let admin = admin(data.clone());
    let page = html(&get(&admin, "/admin/user/new"));
    page.assert_field("post-0-title");

    let form = Submission::new()
        .with("name", "Charles Babbage")
        .with("email", "charles@example.com")
        .with("role", "author")
        .with("active", "on")
        .with("post-0-title", "Passages")
        .with("post-0-views", "7");
    let response = post(&admin, "/admin/user", form);
    assert_eq!(response.location(), Some("/admin/user/3"), "{response:?}");
    assert!(all_posts(&data, &admin).contains(&(3, "Passages".to_string())));
}

#[test]
fn more_children_than_the_form_shows_are_named_and_left_alone() {
    let data = blog();
    let admin = admin_with(
        data.clone(),
        AllowAllUnsafeForDevelopment,
        Inline::new("post").fields(["title"]).max(1).extra(0),
    );
    let page = html(&get(&admin, "/admin/user/1/edit"));
    page.assert_field("post-0-title")
        .assert_text("More posts exist than this form shows");
    assert!(page.field("post-1-title").is_none());
    // A second row is over what the form shows.
    let crafted = ada()
        .with("post-0-pk", "1")
        .with("post-0-title", "a")
        .with("post-1-pk", "3")
        .with("post-1-title", "b");
    assert_eq!(post(&admin, "/admin/user/1", crafted).status(), 422);
}

#[test]
#[should_panic(expected = "names no model the admin shows")]
fn an_inline_of_an_unknown_model_stops_the_admin() {
    admin_with(blog(), AllowAllUnsafeForDevelopment, Inline::new("comment"));
}

#[test]
#[should_panic(expected = "found no column of `user` that points at `post`")]
fn an_inline_with_no_column_pointing_back_stops_the_admin() {
    Admin::new(
        blog(),
        AdminConfig::new()
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .model("post", |m| m.inline(Inline::new("user"))),
    );
}

/// The admin in an application with the CSRF hook: the inline rows ride in
/// the parent's form, so a POST without the token changes no child.
#[test]
fn a_post_without_the_csrf_token_changes_no_child() {
    use brocanth::plugin_api::Plugin;
    use brocanth::plugin_app::AppBuilder;
    use brocanth_admin::{AdminPlugin, AdminSetup};
    use brocanth_middleware::{Csrf, CsrfConfig};
    use brocanth_test::{TestClient, TestRequest};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    let data = blog();
    let config = AdminConfig::new()
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment)
        .model("user", |m| m.inline(Inline::new("post").fields(["title"])));
    let mut app = AppBuilder::new()
        .plugin_hooking(Csrf::new(CsrfConfig::default()))
        .plugin(AdminPlugin::new(AdminSetup::new(data.clone(), config)))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the routes");
    let client = TestClient::new(app);
    let mut form: Vec<(&str, &str)> = vec![
        ("name", "Ada Lovelace"),
        ("email", "ada@example.com"),
        ("role", "editor"),
        ("active", "on"),
        ("post-0-pk", "1"),
        ("post-0-title", "Forged"),
    ];
    let cookie = format!("csrf_token={TOKEN}");

    let refused = client.send(
        TestRequest::post("/admin/user/1")
            .header("cookie", &cookie)
            .form(&form),
    );
    assert_eq!(refused.status(), 403);
    let admin = Admin::new(
        data.clone(),
        AdminConfig::new().discovering(Discovery::new().under("blog::models")),
    );
    assert_eq!(
        title(&data, &admin, 1).as_deref(),
        Some("On the Analytical Engine")
    );

    form.push(("_csrf_token", TOKEN));
    let done = client.send(
        TestRequest::post("/admin/user/1")
            .header("cookie", &cookie)
            .form(&form),
    );
    assert_eq!(done.status(), 303);
    assert_eq!(title(&data, &admin, 1).as_deref(), Some("Forged"));
}

/// A library: authors and books, where an ISBN is unique. The fixture
/// refuses a second book with one ISBN the way a model layer would.
fn library() -> Arc<Fixture> {
    use brocanth_admin::{ColumnDescriptor, ColumnKind, RegistryEntry};
    let author = RegistryEntry::new("library::author", "author")
        .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
        .with(ColumnDescriptor::new("name", ColumnKind::Text));
    let book = RegistryEntry::new("library::book", "book")
        .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
        .with(ColumnDescriptor::new("title", ColumnKind::Text))
        .with(ColumnDescriptor::new("isbn", ColumnKind::Text).unique())
        .with(ColumnDescriptor::new("author_id", ColumnKind::Integer).references("author", "id"));
    Arc::new(
        Fixture::new()
            .model(author)
            .model(book)
            .row(
                "author",
                Row::new().with("id", 1).with("name", "Mary Shelley"),
            )
            .row(
                "book",
                Row::new()
                    .with("id", 1)
                    .with("title", "Frankenstein")
                    .with("isbn", "111")
                    .with("author_id", 1),
            ),
    )
}

#[test]
fn a_row_the_data_source_refuses_keeps_what_was_saved_before_it() {
    let data = library();
    let admin = Admin::new(
        data.clone(),
        AdminConfig::new()
            .at("/admin")
            .unscoped_rows_unsafe_for_development()
            .with_policy(AllowAllUnsafeForDevelopment)
            .model("author", |m| m.inline(Inline::new("book").extra(2))),
    );
    let form = Submission::new()
        .with("name", "Mary Shelley")
        .with("book-0-pk", "1")
        .with("book-0-title", "Frankenstein; or, The Modern Prometheus")
        .with("book-0-isbn", "111")
        .with("book-1-title", "The Last Man")
        .with("book-1-isbn", "222")
        .with("book-2-title", "A copy")
        .with("book-2-isbn", "111");
    let response = post(&admin, "/admin/author/1", form);
    assert_eq!(response.status(), 422);
    let page = html(&response);
    page.assert_text("has already been taken")
        .assert_text("Fix the marked row and save again");
    // The rows before the refused one are saved, and the added one comes
    // back as an existing child, so a second save does not add it twice.
    let book = admin.descriptor("book").unwrap();
    let first = block_on(data.fetch(book, &PrimaryKey::single("id", 1), Scope::Unscoped))
        .unwrap()
        .unwrap();
    assert_eq!(
        first.get("title").map(Value::as_input).as_deref(),
        Some("Frankenstein; or, The Modern Prometheus")
    );
    assert_eq!(data.len("book"), 2);
    assert_eq!(page.field("book-1-pk").unwrap().attr("value"), Some("2"));
    assert_eq!(
        page.field("book-2-isbn").unwrap().attr("value"),
        Some("111")
    );

    // The person fixes the ISBN and saves the form as it came back.
    let again = Submission::new()
        .with("name", "Mary Shelley")
        .with("book-0-pk", "1")
        .with("book-0-title", "Frankenstein; or, The Modern Prometheus")
        .with("book-0-isbn", "111")
        .with("book-1-pk", "2")
        .with("book-1-title", "The Last Man")
        .with("book-1-isbn", "222")
        .with("book-2-title", "A copy")
        .with("book-2-isbn", "333");
    assert_eq!(post(&admin, "/admin/author/1", again).status(), 303);
    assert_eq!(data.len("book"), 3);
}

#[test]
fn a_key_posted_twice_is_refused() {
    let data = blog();
    let admin = admin(data.clone());
    let crafted = ada()
        .with("post-0-pk", "1")
        .with("post-0-title", "Once")
        .with("post-0-views", "1")
        .with("post-1-pk", "1")
        .with("post-1-title", "Twice")
        .with("post-1-views", "1");
    assert_eq!(post(&admin, "/admin/user/1", crafted).status(), 409);
    assert_eq!(
        title(&data, &admin, 1).as_deref(),
        Some("On the Analytical Engine")
    );
}

#[test]
fn an_inline_this_person_may_not_see_leaves_the_others_their_own_columns() {
    use brocanth_admin::{ColumnDescriptor, ColumnKind, RegistryEntry};
    // An award points at its author by name, a book by id. The awards are
    // hidden from this person, so the form shows the books alone.
    let award = RegistryEntry::new("library::award", "award")
        .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
        .with(ColumnDescriptor::new("title", ColumnKind::Text))
        .with(ColumnDescriptor::new("author_name", ColumnKind::Text).references("author", "name"));
    let data = Arc::new(
        Fixture::new()
            .model(
                RegistryEntry::new("library::author", "author")
                    .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
                    .with(ColumnDescriptor::new("name", ColumnKind::Text)),
            )
            .model(
                RegistryEntry::new("library::book", "book")
                    .with(ColumnDescriptor::new("id", ColumnKind::Integer).primary_key())
                    .with(ColumnDescriptor::new("title", ColumnKind::Text))
                    .with(
                        ColumnDescriptor::new("author_id", ColumnKind::Integer)
                            .references("author", "id"),
                    ),
            )
            .model(award)
            .row(
                "author",
                Row::new().with("id", 1).with("name", "Mary Shelley"),
            ),
    );
    let admin = Admin::new(
        data.clone(),
        AdminConfig::new()
            .at("/admin")
            .unscoped_rows_unsafe_for_development()
            .with_policy(policy_fn(|_: &Principal<'_>, request: Request<'_>| {
                request.model != "award"
            }))
            .model("author", |m| {
                m.inline(Inline::new("award")).inline(Inline::new("book"))
            }),
    );
    let page = html(&get(&admin, "/admin/author/1/edit"));
    assert!(page.field("award-0-title").is_none());
    page.assert_field("book-0-title");

    let form = Submission::new()
        .with("name", "Mary Shelley")
        .with("book-0-title", "Frankenstein");
    assert_eq!(post(&admin, "/admin/author/1", form).status(), 303);
    let book = admin.descriptor("book").unwrap();
    let added = block_on(data.fetch(book, &PrimaryKey::single("id", 1), Scope::Unscoped))
        .unwrap()
        .expect("the book");
    assert_eq!(added.get("author_id"), Some(&Value::Int(1)));
}
