//! `admin` and `admin-model` -- what `brocanth create` writes when somebody
//! asks for a backoffice.
//!
//! The module is here rather than in the CLI because what it writes is a
//! statement about this crate: that [`Admin::dispatch`](crate::Admin::dispatch)
//! takes a method, a path, a query and a [`Submission`](crate::Submission),
//! that the admin owns its own path grammar, that the default policy is
//! [`DenyAll`](crate::DenyAll), and that a model with no
//! [`AdminResource`](crate::AdminResource) answers 403. Those move when this
//! crate moves.
//!
//! # Why the routes are page-shaped and not the `RouteSource`
//!
//! `AdminPlugin` contributes the screens as a
//! `RouteSource`, and a generated project cannot mount one: its table holds
//! the project's own synchronous `Handler`, and `RouteCx` needs a context the
//! generated `Ctx` does not lend out. `brocanth-resource` measured the same
//! walls (`brocanth_resource::pages`). So the module writes a `mount` with the
//! shape `Slot::RouterMounts` expects, four routes into the one table, each
//! calling [`Admin::dispatch`](crate::Admin::dispatch). That is forty lines in
//! a file the developer owns, which is also where they say who is asking.
//!
//! # It ships locked
//!
//! The generated admin has no models, the [`DenyAll`](crate::DenyAll) policy
//! and an anonymous principal, so `/admin` answers 403. The generated test
//! asserts exactly that, which proves the routes are in the table and that
//! nothing was opened on the developer's behalf.
//!
//! # Why `admin-model` is listed and not available
//!
//! `brocanth-admin-model` reads rows over a SeaORM `DatabaseConnection`, and
//! the generated `model` plugin opens none: it publishes a URL and no pool.
//! It also takes `brocanth-model` with its default driver, which is Postgres,
//! while the generated config names a SQLite file. A module that wrote
//! `admin_source!` over that would compile and fail on the first request.
//! The reason is on its row until the model plugin opens a pool.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The backoffice, as a choice on the `brocanth create` menu.
///
/// Named `Backoffice` and not `Admin`, which is this crate's own type: a
/// second item under that name makes rustc print fully qualified paths in
/// every error that mentions either. `brocanth-resource` hit that with
/// `Resource`.
pub struct Backoffice;

impl Module for Backoffice {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "admin",
            "a Django-shaped backoffice at /admin, locked until you say who may in",
        )
        .asked_in("doors", "A backoffice at /admin, locked until you open it")
        .krate(
            Dep::brocanth("brocanth-admin")
                // The `RouteSource` is for a host that mounts one. A
                // generated project mounts the page-shaped routes it writes
                // itself, so the plugin half is not compiled.
                .no_default_features(),
        )
        .absent(
            "brocanth-admin",
            "no backoffice; a row is changed through a page somebody wrote, or SQL",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file(
            "src/plugins/admin/mod.rs",
            ADMIN_RS.replace(
                "@TITLE@",
                &format!("{:?}", format!("{} admin", project.title)),
            ),
        );
        s.at(Slot::PluginsMod).line("pub mod admin;");
        // The prefix is the application's decision, so it is written at the
        // call and not inside `mount`.
        s.at(Slot::RouterMounts)
            .line("    crate::plugins::admin::mount(routes, \"/admin\")?;");
        // Two patterns, and the table's own test lists every route.
        s.at(Slot::PagesRoutes).line("/admin");
        s.at(Slot::PagesRoutes).line("/admin/{*rest}");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// The admin over SeaORM, listed and not available.
pub struct BackofficeModels;

impl Module for BackofficeModels {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "admin-model",
            "the admin's rows over SeaORM, one line per entity",
        )
        .asked_in("doors", "")
        .part_of_but_off("admin")
        .because("it is the admin's data source, so there is nothing to read rows for without it")
        .needs(
            "model",
            "it reads rows through the ORM, and without `model` there is none",
        )
        .blocked("the generated model plugin opens no pool, so there is no connection to read")
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // Never reached while the CLI honours `Manifest::blocked`.
        s.refuse(
            "admin-model cannot be scaffolded: `brocanth_admin_model::admin_source!` takes a \
             SeaORM `DatabaseConnection`, and the generated `model` plugin opens none. Write \
             the source by hand over your own connection and pass it to `Admin::new` in \
             `src/plugins/admin/mod.rs`",
        );
    }
}

/// `src/plugins/admin/mod.rs`. `@TITLE@` is the quoted title.
const ADMIN_RS: &str = r##"//! The backoffice, at `/admin`.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! It ships locked, and two answers are yours to give:
//!
//! 1. **Who may open a screen.** The policy is `DenyAll` until
//!    `AdminConfig::with_policy` names another, so every screen answers 403.
//! 2. **Who is asking.** [`principal`] returns an anonymous principal until
//!    it reads your user off the request.
//!
//! It has no models yet. A model needs a data source that reads its rows and
//! a row policy: `AdminConfig::resource(..)`, one per model. A model with no
//! resource answers 403 on every screen. `brocanth_admin::rows` has the
//! worked example, and `brocanth-admin-model` is the data source over
//! SeaORM.
//!
//! The routes are in the one table `src/pages/mod.rs` builds:
//! `pages::mounted` calls [`mount`] with the prefix. A page at `/admin` is
//! therefore a build error, not a race.

use std::future::Future;
use std::sync::Arc;

use brocanth::routing::{BuildError, RouterBuilder, Verb};
use brocanth_admin::policy::Principal;
use brocanth_admin::{Admin, AdminConfig, Fixture, Submission};

use crate::pages::{Ctx, Handler, Response};

/// Every screen of the admin, under `prefix`.
///
/// Four routes: the index and everything under it, for GET and for POST.
/// The admin parses its own paths, so the table gives it the whole path and
/// does not split it into twelve routes.
pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
    let admin = Arc::new(Admin::new(
        // An empty in-memory source: no models, so no rows to read. Put
        // your own `AdminData` here, or `brocanth_admin_model::admin_source!`
        // over your database connection.
        Arc::new(Fixture::new()),
        AdminConfig::new().titled(@TITLE@).at(prefix),
    ));
    let rest = format!("{}/{{*rest}}", prefix.trim_end_matches('/'));
    for pattern in [prefix, rest.as_str()] {
        routes.route(pattern, Verb::Get, screen(Arc::clone(&admin)))?;
        routes.route(pattern, Verb::Post, screen(Arc::clone(&admin)))?;
    }
    Ok(())
}

/// One handler for every screen. The admin reads the path and decides.
fn screen(admin: Arc<Admin>) -> Handler {
    Box::new(move |ctx: &mut Ctx| {
        let ctx = &*ctx;
        // HEAD reaches the GET handler, and the admin knows two verbs.
        let method = match ctx.verb {
            Verb::Post => "POST",
            _ => "GET",
        };
        let form = submission(ctx);
        let who = principal(ctx);
        // `?` on the admin's own failure: a data source that failed, or a
        // template that did not render. A refusal is not one: it is a page.
        let answer = wait(admin.dispatch(who, method, &ctx.path, &ctx.query, &form))?;
        Ok(match answer {
            brocanth_admin::Response::Page { status, html } => Response::new(status, html),
            brocanth_admin::Response::Redirect { status, location } => {
                Response::empty(status).with_header("location", location)
            }
        })
    })
}

/// Who is asking, as the admin's policy sees it.
///
/// Anonymous until this reads your user off the request. When a plugin
/// publishes one, this is one line:
///
/// ```ignore
/// ctx.provided::<CurrentUser, _>()
///     .map_or_else(Principal::anonymous, |user| Principal::new(user))
/// ```
fn principal(_ctx: &Ctx) -> Principal<'_> {
    Principal::anonymous()
}

/// The form a POST sent. Anything that is not urlencoded is an empty form,
/// so the screen shows its errors and writes nothing.
fn submission(ctx: &Ctx) -> Submission {
    let urlencoded = ctx
        .content_type()
        .is_some_and(|t| t.starts_with("application/x-www-form-urlencoded"));
    match ctx.body_text() {
        Some(body) if urlencoded => Submission::from_urlencoded(body),
        _ => Submission::new(),
    }
}

/// Runs the admin's answer to completion on this thread.
///
/// The admin is async because a database is, and a handler in this table is
/// not. Under `serve` the runtime has worker threads, so this tells tokio the
/// thread blocks. Anywhere else, such as a test, the admin's own small
/// executor runs it.
fn wait<F: Future>(future: F) -> F::Output {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        _ => brocanth_admin::rt::block_on(future),
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::{Dispatch, Verb};

    use crate::pages::Ctx;

    /// The admin is in the table, and it answers 403 until somebody opens it.
    #[test]
    fn the_admin_is_mounted_and_ships_locked() {
        let router = crate::pages::router().expect("the table builds");
        let Dispatch::Found { handler, matched } = router.dispatch(Verb::Get, "/admin") else {
            panic!("/admin is not in the table");
        };
        let mut ctx = Ctx::new(Verb::Get, "/admin", matched.params());
        let answer = handler(&mut ctx).expect("the admin answers");
        assert_eq!(answer.status, 403);
    }
}
"##;

/// The `AGENTS.md` section.
const AGENTS: &str = "\n\
    ## The admin\n\
    \n\
    `src/plugins/admin/mod.rs` mounts the backoffice at `/admin`. It ships\n\
    locked: the policy is `DenyAll` and `principal` returns an anonymous\n\
    principal, so every screen answers 403. Do not open it with\n\
    `AllowAllUnsafeForDevelopment` outside a test.\n\
    \n\
    A model reaches the admin through `AdminConfig::resource(..)`: a\n\
    `brocanth_admin::AdminResource` with a `brocanth_policy::Policy` over\n\
    the model's rows. A model with no resource answers 403. Never filter\n\
    rows after a read; the row policy goes into the query.\n";
