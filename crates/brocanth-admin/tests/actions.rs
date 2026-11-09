//! Actions over selected rows, the journal and the record links.
//!
//! Asserted on what a person sees and what the data holds afterwards: the
//! checkboxes and the menu, the confirmation, the redirect and its line,
//! the rows that are gone or changed, and what the journal heard.

use std::sync::{Arc, Mutex};

use brocanth_admin::actions::BulkAction;
use brocanth_admin::admin::Response;
use brocanth_admin::journal::{AdminWrite, Written};
use brocanth_admin::policy::{policy_fn, Action, AllowAllUnsafeForDevelopment, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, AdminData, Discovery, Fixture, PrimaryKey, RowAction, Submission, Value,
};
use brocanth_test::Html;

/// One write, as the journal heard it.
#[derive(Clone, Debug, PartialEq)]
struct Heard {
    who: Option<String>,
    model: String,
    key: String,
    kind: Written,
    fields: Vec<(String, Option<String>, Option<String>)>,
}

type Log = Arc<Mutex<Vec<Heard>>>;

fn admin(config: AdminConfig) -> (Admin, Arc<Fixture>, Log) {
    let data = Arc::new(Fixture::blog());
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let heard = Arc::clone(&log);
    let admin = Admin::new(
        data.clone(),
        config
            .at("/admin")
            .user_label(|_| Some("Ada".to_string()))
            .journal(move |w: &AdminWrite<'_>| {
                heard.lock().unwrap().push(Heard {
                    who: w.who.map(str::to_string),
                    model: w.model.to_string(),
                    key: w.key.to_string(),
                    kind: w.kind,
                    fields: w
                        .fields
                        .iter()
                        .map(|f| (f.name.clone(), f.before.clone(), f.after.clone()))
                        .collect(),
                });
            })
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development(),
    );
    (admin, data, log)
}

fn open() -> AdminConfig {
    AdminConfig::new().with_policy(AllowAllUnsafeForDevelopment)
}

fn send(admin: &Admin, method: &str, path: &str, query: &str, form: &str) -> Response {
    let form = Submission::from_urlencoded(form);
    block_on(admin.dispatch(Principal::anonymous(), method, path, query, &form))
        .unwrap_or_else(|e| panic!("{method} {path}: {e}"))
}

fn html(response: &Response) -> Html {
    Html::new(response.html().expect("a page"))
}

fn post_ids(data: &Fixture) -> Vec<String> {
    let model = brocanth_admin::discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("the post model");
    (1..=3)
        .filter(|id| {
            let key = PrimaryKey::single("id", id);
            block_on(data.fetch(&model, &key, brocanth_admin::Scope::Unscoped))
                .expect("a read")
                .is_some()
        })
        .map(|id| id.to_string())
        .collect()
}

#[test]
fn a_list_with_an_action_has_a_checkbox_per_row_and_a_menu_that_posts() {
    let (admin, _, _) = admin(open().action(BulkAction::delete_selected()));
    let page = html(&send(&admin, "GET", "/admin/post", "", ""));
    page.assert_form_posts_to("/admin/post/actions");
    let boxes: Vec<_> = page
        .fields()
        .into_iter()
        .filter(|f| f.attr("type") == Some("checkbox") && f.attr("name") == Some("pk"))
        .collect();
    assert_eq!(boxes.len(), 2, "one checkbox per post");
    assert!(boxes
        .iter()
        .all(|b| b.attr("form") == Some("admin-actions")));
    assert!(page.text().contains("Delete selected"));
}

#[test]
fn a_list_without_actions_draws_no_checkboxes() {
    let (admin, _, _) = admin(open());
    let page = html(&send(&admin, "GET", "/admin/post", "", ""));
    assert!(page.form_posting_to("/admin/post/actions").is_none());
    assert!(page.field("pk").is_none());
}

#[test]
fn the_first_post_confirms_and_writes_nothing() {
    let (admin, data, log) = admin(open().action(BulkAction::delete_selected()));
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&pk=1&pk=2&pk=999",
    );
    assert_eq!(answer.status(), 200);
    let page = html(&answer);
    page.assert_text("On the Analytical Engine")
        .assert_text("Compilers")
        .assert_text("1 selected record is left out")
        .assert_form_posts_to("/admin/post/actions");
    assert!(page.field("confirm").is_some());
    assert_eq!(post_ids(&data), ["1", "2"], "nothing deleted yet");
    assert!(log.lock().unwrap().is_empty());
}

#[test]
fn the_confirmed_post_deletes_redirects_and_says_how_many() {
    let (admin, data, log) = admin(open().action(BulkAction::delete_selected()));
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&confirm=yes&pk=1&pk=2",
    );
    assert_eq!(answer.status(), 303);
    let location = answer.location().expect("a redirect").to_string();
    assert_eq!(location, "/admin/post?done=delete_selected&n=2");
    assert!(post_ids(&data).is_empty());

    let (path, query) = location.split_once('?').unwrap();
    let list = html(&send(&admin, "GET", path, query, ""));
    list.assert_text("Delete selected: 2 posts.");

    let heard = log.lock().unwrap().clone();
    assert_eq!(heard.len(), 2);
    assert!(heard.iter().all(|h| h.kind == Written::Deleted));
    assert_eq!(heard[0].who.as_deref(), Some("Ada"));
    assert!(heard[0].fields.contains(&(
        "title".into(),
        Some("On the Analytical Engine".into()),
        None
    )));
}

#[test]
fn a_set_action_writes_its_values_and_the_journal_hears_what_changed() {
    let (admin, _, log) = admin(open().model("post", |m| {
        m.action(BulkAction::set(
            "publish",
            "Publish selected",
            [("published", true)],
        ))
    }));
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=publish&confirm=yes&pk=1&pk=2",
    );
    assert_eq!(answer.status(), 303);
    let detail = html(&send(&admin, "GET", "/admin/post/2", "", ""));
    detail.assert_text("Publishedyes");
    // Post 1 was already published, so only post 2 changed a value.
    let heard = log.lock().unwrap().clone();
    let two = heard
        .iter()
        .find(|h| h.key == "2")
        .expect("post 2 was written");
    assert_eq!(two.kind, Written::Updated);
    assert_eq!(
        two.fields,
        vec![(
            "published".into(),
            Some("false".into()),
            Some("true".into())
        )]
    );
}

#[test]
fn a_custom_action_writes_through_the_rules() {
    let (admin, _, _) = admin(open().model("post", |m| {
        m.action(BulkAction::custom(
            "reset",
            "Reset views",
            RowAction::Update,
            |cx| {
                Box::pin(async move {
                    let mut done = 0;
                    for record in cx.selected() {
                        let values = [("views".to_string(), Value::Int(0))];
                        if cx.update(&record.key, &values).await? {
                            done += 1;
                        }
                    }
                    Ok(done)
                })
            },
        ))
    }));
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=reset&confirm=yes&pk=1",
    );
    assert_eq!(answer.location(), Some("/admin/post?done=reset&n=1"));
    let detail = send(&admin, "GET", "/admin/post/1", "", "");
    assert!(!html(&detail).text().contains("1843 "), "views were reset");
}

#[test]
fn a_person_who_may_not_delete_sees_no_menu_and_is_refused() {
    let policy = policy_fn(|_: &Principal<'_>, r: Request<'_>| r.action != Action::Delete);
    let (admin, data, _) = admin(
        AdminConfig::new()
            .with_policy(policy)
            .action(BulkAction::delete_selected()),
    );
    let page = html(&send(&admin, "GET", "/admin/post", "", ""));
    assert!(page.form_posting_to("/admin/post/actions").is_none());
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected&confirm=yes&pk=1",
    );
    assert_eq!(answer.status(), 403);
    assert_eq!(post_ids(&data), ["1", "2"]);
}

#[test]
fn a_model_can_leave_an_action_for_every_model_off() {
    let (admin, _, _) = admin(
        open()
            .action(BulkAction::delete_selected())
            .model("user", |m| m.without_action("delete_selected")),
    );
    let users = html(&send(&admin, "GET", "/admin/user", "", ""));
    assert!(users.form_posting_to("/admin/user/actions").is_none());
    let answer = send(
        &admin,
        "POST",
        "/admin/user/actions",
        "",
        "action=delete_selected&pk=1",
    );
    assert_eq!(answer.status(), 400);
}

#[test]
fn nothing_selected_is_a_page_that_says_so() {
    let (admin, _, _) = admin(open().action(BulkAction::delete_selected()));
    let answer = send(
        &admin,
        "POST",
        "/admin/post/actions",
        "",
        "action=delete_selected",
    );
    assert_eq!(answer.status(), 422);
    html(&answer).assert_text("Tick at least one record");
}

#[test]
#[should_panic(expected = "sets `publishd`, which is not a column")]
fn a_set_action_on_a_column_the_model_lacks_stops_the_admin_at_start() {
    let _ = admin(open().model("post", |m| {
        m.action(BulkAction::set("publish", "Publish", [("publishd", true)]))
    }));
}

#[test]
fn a_record_link_is_drawn_on_the_detail_screen() {
    let (admin, _, _) = admin(open().record_link("History", |r| {
        Some(format!(
            "{}/history?table={}&row={}",
            r.base_path, r.table, r.key
        ))
    }));
    let page = html(&send(&admin, "GET", "/admin/post/1", "", ""));
    page.assert_link_to("/admin/history?table=post&row=1");
}

#[test]
fn the_single_record_screens_tell_the_journal_too() {
    let (admin, _, log) = admin(open());
    let edit = send(
        &admin,
        "POST",
        "/admin/post/2",
        "",
        "title=Compilers%2C+revised&body=A0+and+after.&author_id=2&views=12&published_at=",
    );
    assert_eq!(edit.status(), 303, "{:?}", edit.html());
    let removed = send(&admin, "POST", "/admin/post/1/delete", "", "");
    assert_eq!(removed.status(), 303);

    let heard = log.lock().unwrap().clone();
    assert_eq!(heard[0].kind, Written::Updated);
    assert_eq!(heard[0].model, "post");
    assert!(heard[0].fields.contains(&(
        "title".into(),
        Some("Compilers".into()),
        Some("Compilers, revised".into())
    )));
    assert!(
        !heard[0].fields.iter().any(|f| f.0 == "body"),
        "body did not change"
    );
    assert_eq!(heard[1].kind, Written::Deleted);
    assert_eq!(heard[1].key, "1");
}
