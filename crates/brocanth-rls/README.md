# brocanth-rls

Postgres row-level security, declared in Rust. You declare the roles, the
tables, the grants, the functions and the policies. The crate checks them,
orders them by their dependencies, and writes the migration that turns the
last snapshot into the new one, with its `-- down` part. Postgres only.

The model is Drizzle ORM's `pgRole`, `pgPolicy` and `crudPolicy`, and
drizzle-kit's policy diff. The design and the rejected alternatives are in
[`docs/rls.md`](../../docs/rls.md).

## Getting started

```toml
[dependencies]
brocanth-rls = { git = "https://github.com/DHHwasRight/brocanth", branch = "main" }
```

| Feature | Default | Adds |
|---|---|---|
| `runtime` | on | the definitions, the diff, the migration writer, the catalog check |
| `scaffold` | off | the `rls` module of `brocanth create`; for the CLI only |

In a generated project, `brocanth add rls` writes `src/plugins/rls/mod.rs`
(`schema()`) and the `rls` command. See
[docs/modules/rls.md](../../docs/modules/rls.md).

## Examples

### One tenant per row

```rust
use brocanth_rls::{Policy, Privilege, Role, Schema, Setting, SqlExpr, SqlType, Table};

let member = Role::new("app_member").member("app_login");
let login = Role::existing("app_login");
let tenant = Setting::new("tenant_id", SqlType::BigInt);
let posts = Table::named("posts").force().grant(&member, Privilege::CRUD);
let mine = SqlExpr::col_named("tenant_id").eq(tenant.get());

let schema = Schema::new()
    .role(member.clone())
    .role(login)
    .table(posts.clone())
    .policy(
        Policy::new("posts_tenant", &posts)
            .to(&member)
            .using(mine.clone())
            .with_check(mine),
    );

let sql = schema.create_sql().unwrap();
assert!(sql.contains(r#"ALTER TABLE "posts" FORCE ROW LEVEL SECURITY"#));
assert!(sql.contains(
    r#"CREATE POLICY "posts_tenant" ON "posts" AS PERMISSIVE FOR ALL TO "app_member" USING ("tenant_id" = CAST(NULLIF(current_setting('brocanth.tenant_id', TRUE), '') AS bigint))"#
));
// The role is created before the grant and the policy that name it.
assert!(sql.find("CREATE ROLE").unwrap() < sql.find("CREATE POLICY").unwrap());
```

### Drizzle's `crudPolicy`

`crud(table, role, read, modify)` writes up to four policies. `None` writes
none for that part, and Postgres refuses it.

```rust
use brocanth_rls::{crud, Role, Schema, Setting, SqlExpr, SqlType, Table};

let member = Role::new("member");
let notes = Table::named("notes");
let own = SqlExpr::col_named("owner_id").eq(Setting::new("user_id", SqlType::BigInt).get());
let schema = Schema::new()
    .role(member.clone())
    .table(notes.clone())
    .policies(crud(&notes, &member, Some(own.clone()), Some(own)));
let names: Vec<String> = schema.snapshot().unwrap().tables["notes"]
    .policies
    .keys()
    .cloned()
    .collect();
assert_eq!(
    names,
    ["notes_member_delete", "notes_member_insert", "notes_member_select", "notes_member_update"]
);
```

### A change is a diff, both ways

```rust
use brocanth_rls::{diff, Command, Policy, Schema, SqlExpr, Table};

let posts = Table::named("posts");
let v1 = Schema::new()
    .table(posts.clone())
    .policy(Policy::new("read", &posts).using(SqlExpr::always()))
    .snapshot()
    .unwrap();
let v2 = Schema::new()
    .table(posts.clone())
    .policy(Policy::new("read", &posts).command(Command::Select).using(SqlExpr::always()))
    .snapshot()
    .unwrap();

// `FOR` cannot be altered: the policy is dropped, then created.
assert_eq!(
    diff(&v1, &v2).unwrap().describe(),
    ["drop policy read on posts", "create policy read on posts"]
);
assert!(diff(&v2, &v2).unwrap().is_empty());
```

`migration::make(&schema, dir, name, Format::Sql)` does this against
`migrations/rls.json` and writes `migrations/<UTC>_<name>.sql`, the file
`brocanth_db::migrate` applies.

### A request as its identity

`brocanth-db` sets the identity on the transaction. It ends with the
transaction, so it never reaches the next user of the connection. The
policy and the request name one `Setting` constant, so the key cannot
drift, and a value the policy cannot read as its type is refused:

```rust,no_run
use brocanth_db::sea_query::Query;
use brocanth_db::{Db, DbError, DbIdentity, Exec};
use brocanth_rls::{Setting, SqlType};

const TENANT: Setting = Setting::of("tenant_id", SqlType::BigInt);

async fn titles(db: &Db, tenant: i64) -> Result<Vec<(String,)>, DbError> {
    let me = DbIdentity::new().role("app_member").setting(&TENANT, tenant);
    let q = Query::select().column("title").from("posts").to_owned();
    db.transact_as(&me, async |tx| tx.all(&q).await).await
}
```

### Errors

A definition that cannot become correct SQL is refused before any SQL is
written, with a `help:` line:

```rust
use brocanth_rls::{Policy, Role, Schema, Table};

let posts = Table::named("posts");
let e = Schema::new()
    .table(posts.clone())
    .policy(Policy::new("p", &posts).to(&Role::new("ghost")))
    .snapshot()
    .unwrap_err()
    .to_string();
assert!(e.contains("`ghost`, which is not a role of the schema"));
assert!(e.contains("help: add `Role::new(\"ghost\")`"));
```
