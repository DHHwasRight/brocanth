//! `rls` -- what `brocanth create` writes when somebody asks for Postgres
//! row-level security.
//!
//! The module is here and not in the CLI because what it writes is a
//! statement about this crate: `Schema`, `migration::make`,
//! `catalog::drift`, and `Db::transact_as` with a `DbIdentity`.
//!
//! What a project gets:
//!
//! - `src/plugins/rls/mod.rs`: `schema()`, the roles, tables and policies of
//!   the project, in Rust. The project owns it.
//! - `src/commands/rls.rs`: `rls make <name>` writes the migration and
//!   `migrations/rls.json`; `rls sql` prints the whole schema as SQL;
//!   `rls diff` fails when `schema()` has changes no migration holds, with
//!   no database; `rls check` compares the database with the snapshot.
//! - A section in `AGENTS.md`: a request that reads tenant data runs in
//!   `transact_as`.
//!
//! With `policy`, the file also builds the request's identity
//! (docs/policy-rls-bridge.md, decision 6 and "How a request sets it"):
//! `app_user()`, the one role every request runs as (decision 7), and the
//! `RowSecurity` plugin, which puts `brocanth_rls::identity(&app_user(),
//! &visitor)` on the typed context of each request. With `admin-model` too,
//! `staff` is the identity the admin's data source runs each request as
//! (decision 13).
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, PgPool, Project, Scaffold, Slot,
};

/// Row-level security, as a choice on the `brocanth create` menu.
pub struct Rls;

impl Module for Rls {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "rls",
            "Postgres row-level security declared in Rust; `rls make` writes the migration",
        )
        .contributes::<AgentsSection>(Contribution::new("rls", "how row-level security is declared in Rust and migrated"), true)
        .category(Category::Enhancer)
        .asked_in(
            "data",
            "Row-level security: Postgres hides the rows of other tenants",
        )
        .requires::<PgPool>("the policies live in the database the `db` module opens, and `Db::transact_as` sets the principal")
        .needs(
            "commands",
            "the migration is written by `rls make`, a command of the project's binary that runs `schema()`",
        )
        .adapts_to(
            "model",
            "`rls make` writes a SeaORM migration file, not a SQL file",
        )
        .adapts_to(
            "admin-model",
            "with `policy`, `staff` is the identity the admin runs each request as; without it, AGENTS.md says the admin queries as the login role",
        )
        .adapts_to(
            "policy",
            "the `RowSecurity` plugin puts the request's `DbIdentity`, built from the policy's `Visitor`, on the context",
        )
        .adapts_to(
            "resource",
            "AGENTS.md says how a resource runs each operation as the request's identity",
        )
        .krate(
            Dep::brocanth("brocanth-rls")
                .note("roles, grants and policies in Rust, diffed into migrations"),
        )
        .after("model")
        .absent(
            "brocanth-rls",
            "no row-level security; a query that forgets its WHERE shows every tenant's rows",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/rls/mod.rs", schema_rs(project));
        s.at(Slot::PluginsMod).line("pub mod rls;");
        if project.has("policy") {
            // No setting, so no section: the plugin is built from `()`, as
            // the policy plugin is.
            s.at(Slot::AppPlugins).block(
                "        let builder = builder\n\
                 \x20           .plugin(<crate::plugins::rls::RowSecurity as brocanth::plugin_api::Plugin>::new(()));\n"
                    .to_string(),
            );
            s.at(Slot::PluginsInstalled)
                .line("rls crate::plugins::rls::RowSecurity");
        }
        s.file("src/commands/rls.rs", command_rs(project));
        s.at(Slot::CommandsMod).line("pub mod rls;");
        s.at(Slot::AgentsSections).block(agents(project));
    }
}

fn agents(project: &Project) -> String {
    let policy = project.has("policy");
    let mut extra = String::new();
    if project.has("model") {
        extra.push_str(if policy {
            MODEL_AGENTS_POLICY
        } else {
            MODEL_AGENTS
        });
    }
    if policy && project.has("resource") {
        extra.push_str(RESOURCE_AGENTS);
    }
    if project.has("admin-model") {
        extra.push_str(if policy {
            ADMIN_AGENTS_POLICY
        } else {
            ADMIN_AGENTS
        });
    }
    let identity = if policy {
        IDENTITY_AGENTS_POLICY
    } else {
        IDENTITY_AGENTS
    };
    AGENTS
        .replace("@IDENTITY@", identity)
        .replace("@MODEL@", &extra)
}

/// `src/plugins/rls/mod.rs`: the schema, and with `policy` the request's
/// identity, and with `admin-model` too the staff member's.
fn schema_rs(project: &Project) -> String {
    if !project.has("policy") {
        return SCHEMA_RS
            .replace("@ROLE@", "")
            .replace("@IDENTITY_DOC@", IDENTITY_DOC);
    }
    let mut out = SCHEMA_RS
        .replace("@ROLE@", ".role(app_user())")
        .replace("@IDENTITY_DOC@", IDENTITY_DOC_POLICY);
    out.push_str(REQUEST_RS);
    if project.has("admin-model") {
        out.push_str(STAFF_RS);
    }
    out.push_str(REQUEST_TESTS_RS);
    out
}

fn command_rs(project: &Project) -> String {
    let format = if project.has("model") {
        "SeaOrm"
    } else {
        "Sql"
    };
    let doc = if project.has("model") {
        "a SeaORM migration, because the project has `model`"
    } else {
        "a SQL file for `migrate up`"
    };
    let next = if project.has("model") {
        "List it with `migrate sync`, then apply it with `migrate up`."
    } else {
        "Apply it with `migrate up`."
    };
    COMMAND_RS
        .replace("@NEXT@", next)
        .replace("@FORMAT_DOC@", doc)
        .replace("@FORMAT@", format)
        .replace("@URL@", URL)
}

/// `src/plugins/rls/mod.rs`.
const SCHEMA_RS: &str = r#"//! Row-level security: the roles, tables and policies of this database.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! Declare the rules here, then run `brocanth rls make <name>`. It compares
//! `schema()` with `migrations/rls.json` and writes a migration with the
//! difference. `brocanth migrate up` applies it. See `docs/rls.md` in
//! brocanth for the design.
//!
//! ```ignore
//! use brocanth_rls::{Policy, Privilege, Role, Setting, SqlExpr, SqlType, Table};
//!
//! /// The tenant of the request: the policy reads it, the request sets it.
//! pub const TENANT: Setting = Setting::of("tenant_id", SqlType::BigInt);
//!
//! let member = Role::new("app_user").member("app_login");
//! // With `model`, name the table and column by the entity: a typo does
//! // not compile. `Table::named("posts")` is the string escape hatch.
//! // Forced by default: the owner obeys the policies too.
//! let posts = Table::of(post::Entity).grant(&member, Privilege::CRUD);
//! let mine = SqlExpr::column(post::Column::TenantId).eq(TENANT.get());
//! Schema::new()
//!     .role(member.clone())
//!     .table(posts.clone())
//!     .policy(Policy::new("posts_tenant", &posts).to(&member).using(mine.clone()).with_check(mine))
//! ```
//!
@IDENTITY_DOC@
use brocanth_rls::Schema;

/// Every role, table, function and policy row-level security needs.
pub fn schema() -> Schema {
    Schema::new()@ROLE@
}
"#;

/// The end of the module doc without `policy`: the request builds its
/// identity by hand.
const IDENTITY_DOC: &str = r#"//! A request then runs as its identity, inside one transaction:
//!
//! ```ignore
//! let me = brocanth_db::DbIdentity::new().role("app_user").setting(&TENANT, tenant_id);
//! let posts: Vec<(i64, String)> = ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
//! ```
"#;

/// The end of the module doc with `policy`: the plugin below builds the
/// identity.
const IDENTITY_DOC_POLICY: &str = r#"//! A request runs as its identity, inside one transaction. The
//! `RowSecurity` plugin below builds it from the policy's `Visitor`: the
//! role [`app_user`] and one setting per attribute (`brocanth.account_id`,
//! `brocanth.admin`). [`identity`] reads it off the request:
//!
//! ```ignore
//! let me = crate::plugins::rls::identity(ctx).expect("dispatched");
//! let posts: Vec<(i64, String)> = ctx.db().transact_as(me, async |tx| tx.all(&q).await).await?;
//! ```
"#;

/// The rest of `src/plugins/rls/mod.rs` with `policy`: the role, the
/// subject's attributes, and the plugin that puts the request's identity
/// on the context.
const REQUEST_RS: &str = r#"
// ---------------------------------------------------------------------------
// Who a request runs as
// ---------------------------------------------------------------------------

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth::plugin_hooks::{Request, RequestState};
use brocanth_policy::{Attr, AttrKind, AttrType, Attributes, Subject};
use brocanth_rls::{DbIdentity, Role};

use crate::plugins::policy::Visitor;

/// The one role every request under a policy runs as. "Admin" is a setting
/// the policy guards on, not a role: a role per kind of user needs a grant
/// and a migration per kind.
///
/// `schema()` creates it. The login the application connects with must be
/// a member, or `SET ROLE` fails: add `.member("<your login role>")` here
/// when the application does not connect as a superuser.
pub fn app_user() -> Role {
    Role::new("app_user")
}

/// The attributes of `Visitor` a database policy may read: one setting
/// each, `brocanth.<field>`, of the field's type.
///
/// Keep it in step with `Visitor` in `src/plugins/policy/mod.rs`: one entry
/// per field a rule reads, in `ATTRS` and in `attributes`. A field that is
/// not here never reaches the database.
impl Subject for Visitor {
    const ATTRS: &'static [Attr] = &[
        Attr {
            key: "account_id",
            kind: AttrKind::BigInt,
            optional: true,
        },
        Attr {
            key: "admin",
            kind: AttrKind::Boolean,
            optional: false,
        },
    ];

    fn attributes(&self) -> Attributes {
        Attributes::new()
            .with("account_id", self.account_id.attr_value())
            .with("admin", self.admin.attr_value())
    }
}

/// The request's database identity, on its typed context.
///
/// A newtype: `ContextItem` is brocanth's trait and `DbIdentity` is
/// `brocanth-db`'s type, so only a type of this crate can carry it.
pub struct Identity(pub DbIdentity);

impl ContextItem for Identity {}

/// Who the request `ctx` runs as in the database: pass it to
/// `transact_as`. `None` on a `Ctx` built by hand.
#[allow(dead_code)]
pub fn identity(ctx: &crate::pages::Ctx) -> Option<&DbIdentity> {
    ctx.provided::<Identity, _>().map(|identity| &identity.0)
}

/// Puts the request's [`Identity`] on the context:
/// `brocanth_rls::identity(&app_user(), &visitor)`, the role and every key
/// of `Visitor::ATTRS`. An absent value is set to `''`, which a policy
/// reads as `NULL`.
///
/// The visitor is the one a request hook of yours put in the request
/// state (`state.insert(visitor)`), after it read the session. Without
/// one, the request is anonymous.
pub struct RowSecurity;

impl Plugin for RowSecurity {
    /// No setting: the role is code, in [`app_user`].
    type Config = ();
    type Provides = ProvidesContext<Identity>;
    const NAME: &'static str = "rls";

    fn new(_config: ()) -> Self {
        Self
    }

    /// Builds the anonymous identity once, at start: a role name or a key
    /// that `SET ROLE` or `set_config` would refuse stops the start, not
    /// the first request.
    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        brocanth_rls::identity(&app_user(), &Visitor::anonymous())
            .statements()
            .map(drop)
            .map_err(|e| PluginError::new(format!("[rls] the request identity: {e}")))
    }

    /// A context with no request: the anonymous identity.
    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(Identity(brocanth_rls::identity(
            &app_user(),
            &Visitor::anonymous(),
        )));
    }

    fn provide_for_request(
        &self,
        _request: &Request,
        state: &RequestState,
        cx: &mut ContextBuilder,
    ) {
        let anonymous = Visitor::anonymous();
        let who = state.get::<Visitor>().unwrap_or(&anonymous);
        cx.insert(Identity(brocanth_rls::identity(&app_user(), who)));
    }
}
"#;

/// The test of the request identity, last in the file.
const REQUEST_TESTS_RS: &str = r#"
#[cfg(test)]
mod tests {
    use super::*;

    /// Every attribute is set, also when it is absent: `''` reads as
    /// `NULL`, so "absent" is "anonymous" in the database too.
    #[test]
    fn an_anonymous_visitor_sets_every_key() {
        let me = brocanth_rls::identity(&app_user(), &Visitor::anonymous());
        assert_eq!(me.role_name(), Some("app_user"));
        let keys: Vec<&str> = me.settings().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["account_id", "admin"]);
        assert_eq!(me.settings()[0].1, "");
        me.statements().expect("the identity is one Postgres takes");
    }
}
"#;

/// With `admin-model`: the identity the admin's data source runs each
/// request as.
const STAFF_RS: &str = r#"
/// Who a backoffice request runs as in the database: the member of staff,
/// as a `Visitor` with `admin` set. The admin's own row policy still goes
/// into every `WHERE`; the database checks the same rows under the
/// `admin` guard. An anonymous principal gets the anonymous identity: the
/// admin refuses it before it reads a row.
///
/// `src/plugins/admin/mod.rs` passes it to `Models::as_identity`.
pub fn staff(principal: brocanth_admin::Principal<'_>) -> DbIdentity {
    let who = Visitor {
        admin: !principal.is_anonymous(),
        ..Visitor::anonymous()
    };
    brocanth_rls::identity(&app_user(), &who)
}
"#;

const COMMAND_RS: &str = r#"//! `rls` -- the row-level security of `src/plugins/rls/mod.rs`, as
//! migrations.
//!
//! `rls make <name>` compares `schema()` with `migrations/rls.json` and
//! writes the difference as a migration, with its `-- down` part. It
//! applies nothing: `migrate up` does. `rls sql` prints the whole schema as
//! SQL. `rls diff` fails when `schema()` differs from the snapshot; it needs
//! no database, so CI can run it. `rls check` compares the database in
//! `[db] url` with the snapshot.

use std::path::Path;

use brocanth_command::{anyhow, clap, cmd, Command, Context};
use brocanth_db::{Db, DbConfig};
use brocanth_rls::migration::{self, Format};

/// The migration file `rls make` writes: @FORMAT_DOC@.
const FORMAT: Format = Format::@FORMAT@;

/// What to run after `rls make`.
const NEXT: &str = "@NEXT@";

/// Write, print or check the row-level security migrations
#[cmd]
#[derive(clap::Args)]
pub struct Rls {
    #[command(subcommand)]
    action: Action,
}

#[derive(clap::Subcommand)]
enum Action {
    /// Write migrations/<UTC time>_<NAME> with what changed since migrations/rls.json
    Make {
        /// The name of the migration, in snake case: add_post_policies
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Print the SQL that creates the whole schema
    Sql,
    /// Fail when schema() has changes that migrations/rls.json does not hold
    Diff,
    /// Compare the database in `[db] url` with migrations/rls.json
    Check,
}

impl Command for Rls {
    fn run(self, ctx: &mut Context) -> anyhow::Result<()> {
        std::env::set_current_dir(ctx.root())?;
        let dir = Path::new("migrations");
        let schema = crate::plugins::rls::schema();
        match self.action {
            Action::Make { name } => match migration::make(&schema, dir, &name, FORMAT)? {
                None => println!("Nothing to write: migrations/rls.json matches schema()."),
                Some(made) => {
                    println!("wrote {}", made.path.display());
                    for step in made.steps {
                        println!("  {step}");
                    }
                    println!("{NEXT}");
                }
            },
            Action::Sql => print!("{}", schema.create_sql()?),
            Action::Diff => {
                let steps = migration::pending(&schema, dir)?;
                if steps.is_empty() {
                    println!("migrations/rls.json matches schema().");
                } else {
                    for step in &steps {
                        println!("  {step}");
                    }
                    anyhow::bail!(
                        "schema() differs from migrations/rls.json\n\
                         help: run `rls make <name>` and commit the migration with migrations/rls.json"
                    );
                }
            }
            Action::Check => {
                let snapshot = migration::read_snapshot(dir)?;
                let config = database()?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                let (drift, owned) = runtime.block_on(async {
                    let db = Db::connect(&config).await?;
                    let drift = brocanth_rls::catalog::drift(&db, &snapshot).await;
                    let owned = brocanth_rls::catalog::owned_unforced(&db, &snapshot).await;
                    db.close().await;
                    anyhow::Ok((drift?, owned?))
                })?;
                // Not drift: the snapshot says `.no_force()`. But the login
                // bypasses the policies of a table it owns.
                for line in &owned {
                    eprintln!("warning: {line}");
                }
                if drift.is_empty() {
                    println!("The database matches migrations/rls.json.");
                } else {
                    for line in &drift {
                        println!("{line}");
                    }
                    anyhow::bail!(
                        "the database differs from migrations/rls.json\n\
                         help: run `migrate up`, or `rls make <name>` after a change to schema()"
                    );
                }
            }
        }
        Ok(())
    }
}
@URL@"#;

const URL: &str = r#"
/// The pool's configuration: `[db]`, as the server reads it, through every
/// settings layer: the default, the module file, the files, the
/// environment and the flags.
fn database() -> anyhow::Result<DbConfig> {
    let layers = crate::plugins::layers().map_err(anyhow::Error::msg)?;
    let settings = crate::plugins::db_settings(&layers).map_err(anyhow::Error::msg)?;
    Ok(settings.config())
}
"#;

const AGENTS: &str = r#"
## Row-level security

`src/plugins/rls/mod.rs` declares the roles, tables and policies in Rust.
Do not write `CREATE POLICY` in a migration by hand. Change `schema()`,
then run `brocanth rls make <name>` and read the file it writes. Commit it
with `migrations/rls.json`.

A request that reads or writes a table with a policy runs in one
transaction, as its identity. @IDENTITY@
The role and the values end with the transaction, so they never reach the
next request on the same connection. A statement on `ctx.db()` outside
`transact_as` runs as the login role, and the policies see no value. A
table is forced by default. On a table declared `.no_force()` the owner
bypasses the policies: when the login role owns the tables, a statement
outside `transact_as` returns every row.
@MODEL@
Never connect as a superuser: a superuser bypasses every policy.
"#;

/// How a request gets its identity, without `policy`: by hand.
const IDENTITY_AGENTS: &str = r#"Set each value through the `Setting`
constant the policy reads, so the key cannot drift:

```rust
let me = brocanth_db::DbIdentity::new().role("app_user").setting(&TENANT, tenant_id);
ctx.db().transact_as(&me, async |tx| tx.all(&q).await).await?;
```
"#;

/// How a request gets its identity, with `policy`: from the plugin.
const IDENTITY_AGENTS_POLICY: &str = r#"The `RowSecurity` plugin in
`src/plugins/rls/mod.rs` builds it for each request:
`brocanth_rls::identity(&app_user(), &visitor)`, the role `app_user` and
one setting per attribute of `Visitor`. Do not build a `DbIdentity` by
hand: every key is always set, and an absent value reads as `NULL`.
A request hook puts the request's `Visitor` in the request state; without
one, the request is anonymous. With `sqlx`:

```rust
let me = crate::plugins::rls::identity(ctx).expect("dispatched");
ctx.db().transact_as(me, async |tx| tx.all(&q).await).await?;
```

A field a database policy reads is an entry in `impl Subject for Visitor`
in that file: add it there when you add it to `Visitor`.
"#;

const MODEL_AGENTS: &str = r#"
A SeaORM query takes the identity the same way. `Orm::transact_as` runs
the same statements in a SeaORM transaction:

```rust
orm.transact_as(&me, async |txn| post::Entity::find().all(txn).await).await?;
```

A query on the `Orm` itself runs as the login role, and on a `FORCE`
table it sees no row.
"#;

/// With `model` and `policy`: the SeaORM call site, with the request's
/// identity.
const MODEL_AGENTS_POLICY: &str = r#"
A SeaORM query takes the same identity. `Orm::transact_as` runs the same
statements in a SeaORM transaction:

```rust
let orm = ctx.provided::<brocanth_model::Orm, _>().expect("dispatched");
orm.transact_as(me, async |txn| post::Entity::find().all(txn).await).await?;
```

A query on `ctx.orm()` itself runs as the login role, and on a `FORCE`
table it sees no row.
"#;

/// With `resource` and `policy`: a resource runs as the request too.
const RESOURCE_AGENTS: &str = r#"
A resource of the records API runs each operation in one session as the
request's identity. Its data source takes
`type Identity = Option<brocanth_db::DbIdentity>` and opens a `Tx` that
assumes it in `ResourceData::session`. Its `PageSubject` reads the
identity off the request:

```rust
fn identity(&self, ctx: &Ctx) -> Result<Option<DbIdentity>, Failure> {
    Ok(crate::plugins::rls::identity(ctx).cloned())
}
```
"#;

/// With `admin-model` and `policy`: the admin runs as the staff member.
const ADMIN_AGENTS_POLICY: &str = r#"
The admin's data source runs each request as `crate::plugins::rls::staff`:
`Models::as_identity` in `src/plugins/admin/mod.rs`. A table the admin
shows needs a grant to `app_user` (`Table::grant`), and a policy that
admits staff (`who.admin => always`), or the admin sees no row of it.
"#;

const ADMIN_AGENTS: &str = r#"
The admin's data source queries the `Orm` with no identity, as the login
role. On a table with a policy it sees only what the login may see, and
on a `FORCE` table nothing. Give the admin its own policy (`TO` the login
role, or a staff role it is granted), or keep the tables it edits out of
row-level security.
"#;
