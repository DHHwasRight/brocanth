//! The admin's screens served through the tab data contract, with RBAC per
//! tab: the shape of the backoffice binary, in one test process.
//!
//! The application mounts the admin's plugin half. The admin's source goes
//! through `Contracted`, a JSON codec and `InProcess`, so every call
//! crosses the codec. The principal comes from a request header, as a
//! session would give it.

use std::sync::Arc;

use brocanth::plugin_api::Plugin as _;
use brocanth::plugin_app::AppBuilder;
use brocanth::plugin_hooks::NoHooks;
use brocanth::plugin_hooks::{Request, RequestState};
use brocanth_admin::data::{AdminData, Scope};
use brocanth_admin::model::PrimaryKey;
use brocanth_admin::policy::{policy_fn, AllowAllUnsafeForDevelopment, Level, Policy, Principal};
use brocanth_admin::rt::block_on;
use brocanth_admin::{discover, AdminConfig, AdminPlugin, AdminSetup, Discovery, Fixture};
use brocanth_backoffice::{Claim, Contracted, InProcess, Json};
use brocanth_test::{TestClient, TestRequest};

/// A member of staff, as the application's session would give one.
struct Staff {
    id: &'static str,
    superuser: bool,
    grants: &'static [&'static str],
}

impl Staff {
    /// `brocanth_auth::Staff::allows`, for a test: a higher level holds
    /// the lower ones.
    fn holds(&self, resource: &str, level: Level) -> bool {
        self.superuser
            || Level::ALL
                .into_iter()
                .filter(|held| *held >= level)
                .any(|held| self.grants.contains(&held.on(resource).as_str()))
    }
}

static READER: Staff = Staff {
    id: "1",
    superuser: false,
    grants: &["post.read", "user.read"],
};
static EDITOR: Staff = Staff {
    id: "2",
    superuser: false,
    grants: &["post.edit", "user.read"],
};
static ROOT: Staff = Staff {
    id: "3",
    superuser: true,
    grants: &[],
};

/// The member of staff the `x-staff` header names.
fn principal<'a>(request: &'a Request, _: &'a RequestState) -> Principal<'a> {
    let who = request
        .headers()
        .get("x-staff")
        .and_then(|v| v.to_str().ok());
    match who {
        Some("reader") => Principal::new(&READER),
        Some("editor") => Principal::new(&EDITOR),
        Some("root") => Principal::new(&ROOT),
        _ => Principal::anonymous(),
    }
}

/// RBAC per tab, as `StaffPolicy` decides it: the level each action needs
/// on the model's resource.
fn rbac() -> impl Policy {
    policy_fn(|who: &Principal<'_>, request| {
        let Some(staff) = who.downcast::<Staff>() else {
            return false;
        };
        match request.action.level() {
            None => true,
            Some(level) => staff.holds(request.model, level),
        }
    })
}

/// The claim the contract carries: the member of staff and their grants.
fn claim(principal: &Principal<'_>) -> Claim {
    match principal.downcast::<Staff>() {
        Some(staff) => Claim::staff(staff.id, staff.superuser, staff.grants.iter().copied()),
        None => Claim::from_principal(principal),
    }
}

fn client(
    data: Arc<Fixture>,
    policy: impl Policy,
    enforce: bool,
) -> TestClient<impl Sized + 'static, NoHooks> {
    let config = AdminConfig::new()
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(policy)
        .data_through(move |data| {
            let tabs = if enforce {
                InProcess::new(data).enforcing_grants()
            } else {
                InProcess::new(data)
            };
            Contracted::new(Json::new(tabs)).claim(claim).shared()
        });
    let mut app = AppBuilder::new()
        .plugin(AdminPlugin::new(
            AdminSetup::new(data, config).principal(principal),
        ))
        .build()
        .expect("an application");
    app.mount_routes::<AdminPlugin>("/admin")
        .expect("the routes");
    TestClient::new(app)
}

fn title(data: &Fixture, id: i64) -> Option<String> {
    let model = discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("post");
    block_on(data.fetch(&model, &PrimaryKey::single("id", id), Scope::Unscoped))
        .expect("a fetch")
        .and_then(|row| row.get("title").map(ToString::to_string))
}

/// The edit form of post 1, as a browser posts it.
fn edit(title: &str) -> Vec<(&str, &str)> {
    vec![
        ("title", title),
        ("body", "Notes."),
        ("author_id", "1"),
        ("published", "on"),
        ("views", "1843"),
        ("published_at", "1843-10-01"),
    ]
}

#[test]
fn a_reader_sees_the_list_through_the_contract_and_cannot_write() {
    let data = Arc::new(Fixture::blog());
    let client = client(Arc::clone(&data), rbac(), true);

    let list = client.send(TestRequest::get("/admin/post").header("x-staff", "reader"));
    assert_eq!(list.status(), 200);
    list.html()
        .assert_text("On the Analytical Engine")
        .assert_text("Compilers");

    let refused = client.send(
        TestRequest::post("/admin/post/1")
            .header("x-staff", "reader")
            .form(&edit("Forged")),
    );
    assert_eq!(refused.status(), 403);
    assert_eq!(title(&data, 1).as_deref(), Some("On the Analytical Engine"));
}

#[test]
fn an_editor_writes_through_the_contract() {
    let data = Arc::new(Fixture::blog());
    let client = client(Arc::clone(&data), rbac(), true);

    let saved = client.send(
        TestRequest::post("/admin/post/1")
            .header("x-staff", "editor")
            .form(&edit("Edited through the contract")),
    );
    assert_eq!(saved.status(), 303);
    assert_eq!(
        title(&data, 1).as_deref(),
        Some("Edited through the contract")
    );
}

#[test]
fn a_grant_is_per_tab_and_a_superuser_holds_every_tab() {
    let data = Arc::new(Fixture::blog());
    let client = client(Arc::clone(&data), rbac(), true);

    // The editor holds `user.read` and not `user.edit`.
    let user_form = client.send(TestRequest::get("/admin/user/1/edit").header("x-staff", "editor"));
    assert_eq!(user_form.status(), 403);
    let root_form = client.send(TestRequest::get("/admin/user/1/edit").header("x-staff", "root"));
    assert_eq!(root_form.status(), 200);

    let nobody = client.send(TestRequest::get("/admin/post"));
    assert_eq!(nobody.status(), 403);
}

#[test]
fn the_data_side_refuses_a_write_a_lax_policy_let_through() {
    // A policy that lets everybody in: the second check is the data side's.
    let data = Arc::new(Fixture::blog());
    let client = client(Arc::clone(&data), AllowAllUnsafeForDevelopment, true);

    let refused = client.send(
        TestRequest::post("/admin/post/1")
            .header("x-staff", "reader")
            .form(&edit("Forged")),
    );
    assert_ne!(refused.status(), 303, "the write must not succeed");
    assert_eq!(title(&data, 1).as_deref(), Some("On the Analytical Engine"));

    // Without the data side's check, the same lax policy writes: the check
    // above is what refused it.
    let open = client_without_check(Arc::clone(&data));
    let written = open.send(
        TestRequest::post("/admin/post/1")
            .header("x-staff", "reader")
            .form(&edit("Written")),
    );
    assert_eq!(written.status(), 303);
    assert_eq!(title(&data, 1).as_deref(), Some("Written"));
}

fn client_without_check(data: Arc<Fixture>) -> TestClient<impl Sized + 'static, NoHooks> {
    client(data, AllowAllUnsafeForDevelopment, false)
}
