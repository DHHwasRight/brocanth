# Row-level security in Rust

Written 11 November 2026, with `brocanth-rls`. Postgres only.

Postgres row-level security (RLS) filters rows in the database. A query that
forgets its `WHERE` still sees only the rows of its principal. `brocanth-rls`
declares the roles, the functions, the table switches, the grants and the
policies in Rust. The crate resolves the order between them, writes the
migration and reverses it. Nobody writes `CREATE POLICY` by hand.

The model is Drizzle ORM (`pgRole`, `pgPolicy`, `.enableRLS()`, `crudPolicy`,
drizzle-kit's policy diff). Atlas's `policy` blocks were the second reference.
No Rust library does this today: the crates on crates.io (`umbral-rls`,
`rlsspec`, `demesne-postgres`) are 0.0.x or 0.1 experiments.

## 1. The API

```rust
use brocanth_rls::{crud, Command, Function, Policy, Privilege, Role, Schema, Setting, SqlExpr, SqlType, Table};

let member = Role::new("app_member").member("app_login");    // NOLOGIN, granted to the login
let tenant = Setting::new("tenant_id", SqlType::BigInt);     // brocanth.tenant_id
let posts = Table::of(post::Entity).force();                // ENABLE + FORCE

let mine = SqlExpr::column(post::Column::TenantId).eq(tenant.get());

let schema = Schema::new()
    .role(member.clone())
    .table(posts.clone().grant(&member, Privilege::CRUD))
    .policy(
        Policy::new("posts_tenant", &posts)
            .to(&member)
            .using(mine.clone())
            .with_check(mine),
    );
```

| Rust | Postgres | Default |
|---|---|---|
| `Role::new(name)` | `CREATE ROLE "name" NOLOGIN` | `INHERIT`, no `CREATEDB`, no `CREATEROLE` |
| `Role::existing(name)` | nothing: a role made elsewhere (the login, `postgres`) | |
| `.member(login)` | `GRANT "name" TO "login"` | |
| `Table::of(Entity)` / `Table::of_in(s, Entity)`; strings: `Table::named(name)` / `Table::named_in(s, name)` | `ALTER TABLE … ENABLE ROW LEVEL SECURITY` | enabled, not forced |
| `.force()` | `ALTER TABLE … FORCE ROW LEVEL SECURITY` | |
| `.grant(&role, privileges)` | `GRANT SELECT, … ON … TO …` | |
| `Policy::new(name, &table)` | `CREATE POLICY` | `AS PERMISSIVE FOR ALL TO PUBLIC` |
| `.restrictive()`, `.command(Command::Select)`, `.to(&role)` | `AS RESTRICTIVE`, `FOR SELECT`, `TO "role"` | |
| `.using(expr)`, `.with_check(expr)` | `USING (…)`, `WITH CHECK (…)` | none |
| `crud(&table, &role, read, modify)` | four policies, as Drizzle's `crudPolicy` | |
| `Setting::new(key, type)` | `CAST(NULLIF(current_setting('brocanth.key', TRUE), '') AS type)` | |
| `Function::new(name, returns)` | `CREATE OR REPLACE FUNCTION … LANGUAGE sql STABLE` | `SECURITY INVOKER` |

`crud(table, role, read, modify)` follows Drizzle: `read` and `modify` are
`Option<SqlExpr>`. `None` writes no policy for that part, so the part is
refused (RLS denies what no policy allows). `Some(expr)` writes
`<table>_<role>_select` (`USING read`), `_insert` (`WITH CHECK modify`),
`_update` (`USING modify WITH CHECK modify`) and `_delete` (`USING modify`).

### The expression type

`SqlExpr` wraps a `sea_query::Expr`, the builder `brocanth-db` already
re-exports. It adds one thing: the set of definitions the expression uses.
`setting.get()` records the setting. `function.call(args)` records the
function. `and`, `or`, `eq` and the other combinators merge the sets. So the
policy knows what it needs without a parser.

- A column is `SqlExpr::column(post::Column::TenantId)` (any sea-query
  `Iden`), written `"tenant_id"`. `SqlExpr::col_named("tenant_id")` is the
  string escape hatch.
- A value is `SqlExpr::val(v)`. DDL takes no bind parameter, so sea-query
  writes the value as a literal with its own escaping (`E'O\'Brien'`, a
  line break as `\n`). The value never passes through `format!`, and a
  value cannot put a line `-- down` into the migration file.
- `SqlExpr::exists(select)` takes a `sea_query::SelectStatement`, for a
  membership test. `.uses(&other)` adds the definitions of an expression the
  select contains, because sea-query cannot report them.
- `SqlExpr::raw(sea_query::Expr)` is the escape hatch. Its dependencies are
  what you add with `.uses`.

Every identifier (role, table, policy, function, schema) is double-quoted
with `"` doubled inside. A name longer than 63 bytes is refused: Postgres
truncates it, and the diff would then compare two different names forever.
A type name (`SqlType::Custom`) is checked against
`[A-Za-z0-9_ .,()\[\]]`. A setting key is `[a-z][a-z0-9_]*`.

## 2. The dependency graph

Each definition is a node. An edge points from a node to a node it needs.

```
Role ──────────────┐
  ▲                ▼
  │      Grant ─▶ Table (enable, force)
  │                ▲
Policy ────────────┘
  │
  ▼
Function ─▶ Function
  (Setting: no node; it is part of the expression)
```

- A policy needs its table (RLS on), each managed role in its `TO`, and
  each function its expressions call.
- A grant needs its table and its role.
- A role's membership needs the role.
- A function needs each function its body calls. A cycle is refused, with
  the names on it.
- A setting is not a node: it creates nothing in Postgres.
  `Schema::settings()` lists the settings the expressions read, and
  `Schema::check_identity` refuses a `DbIdentity` key no policy reads: a
  typo would otherwise read as `NULL` and admit no row, with no error.

`Schema::snapshot()` checks the graph: a policy on an undeclared table, a
`TO` role that is neither `Role::new` nor `Role::existing`, a function
cycle, two policies with one name on one table. Each refusal carries a
`help:` line.

Creates run in topological order. Ties break by kind (role, function,
table, grant, policy), then by name, so the output is stable. Drops run in
the reverse order of the old graph. So a policy is dropped before its role
and its function, which Postgres requires. Drizzle drops roles before
policies, and that migration fails; this order does not.

## 3. How a migration is made

### Decision: diff against the last snapshot, not against the live catalog

`rls make <name>` compares `Schema::snapshot()` with
`migrations/rls.json`, the snapshot of the last run. It writes
`migrations/<UTC>_<name>.sql` and the new `rls.json`. It does not connect
to a database.

Reasons:

1. **Postgres rewrites expressions.** `pg_policies.qual` is `pg_get_expr`
   output: added parentheses, `::text` casts, folded keywords. A text
   compare with the catalog reports a change on every run. drizzle-kit
   `push` hides this by not comparing expressions at all, which misses real
   changes. A snapshot compares our text with our text.
2. **The same shape as the rest of brocanth.** `brocanth-migrate` diffs the
   models against `migrations/schema.json`. The generator proposes a file,
   the developer reads and commits it, and `migrate up` applies it. A
   migration that reads the catalog proposes something different on each
   machine.
3. **No database at generation time.** CI and a laptop without Postgres
   write the same file.

The catalog is still read, for a different question: `rls check` compares
the structural fields (roles, `relrowsecurity`, `relforcerowsecurity`,
policy names, `permissive`, `cmd`, `roles`, grants) with the snapshot. It
reports drift. It does not compare expressions, for reason 1.

Rejected:

- **Diff against the live catalog** (Atlas). Atlas normalizes both sides
  through a scratch database (`--dev-url`). That needs a second database at
  generation time and a round trip per policy. It is a later option for
  `rls check`, not for `make`.
- **Only a full `CREATE` script, no diff.** Policies are stateless, so
  drop-all-and-recreate is correct. It is not safe: between the drop and the
  create, a table with RLS on and no policy refuses every row, and a table
  with RLS off shows every row. A diff changes only what changed.

### The steps

For each node, the diff decides one of: unchanged, create, drop, alter,
recreate (drop then create).

| Node | Alter in place | Recreate |
|---|---|---|
| Role | `ALTER ROLE … WITH [NO]INHERIT [NO]CREATEDB [NO]CREATEROLE`; membership `GRANT`/`REVOKE` | never |
| Function | body or volatility: `CREATE OR REPLACE` | arguments or return type |
| Table | `ENABLE`/`DISABLE`, `FORCE`/`NO FORCE` | never |
| Grant | `REVOKE ALL`, then `GRANT` | never |
| Policy | `ALTER POLICY … TO … USING … WITH CHECK …` | `AS` or `FOR` changed, the table changed, or `USING`/`WITH CHECK` removed (`ALTER POLICY` cannot clear one) |

A recreate spreads to the nodes that depend on it: `DROP FUNCTION` fails
while a policy calls the function, so those policies are recreated too.

Output order: every drop (old graph, reverse topological order), then every
create and alter (new graph, topological order).

The `-- down` part is not the inverse of each step. It is the diff from the
new snapshot to the old one, so it follows the same rules and the same
order.

A rename is a drop and a create. drizzle-kit asks whether a pair is a
rename. Here a policy holds no data, so a drop plus a create loses nothing,
and the migration runs in one transaction, so no request sees the gap. A
role rename would lose grants made outside this crate; rename a role by
hand.

### Roles are cluster-wide

`CREATE ROLE` is not per database. Two databases on one server, or a
re-created test database, meet a role that exists. So a role is created
with `DO $rls$ … IF NOT EXISTS (SELECT FROM pg_roles …) THEN CREATE ROLE …`
and dropped with `DROP ROLE IF EXISTS`. A role that still owns objects or
holds privileges outside the schema fails to drop: Postgres names them.

`BYPASSRLS` and `SUPERUSER` are not offered. A superuser bypasses every
policy, `FORCE` included. Do not connect the application as a superuser.

### Where the file goes

The `db` module's SQL migrations (`docs/migrations.md` §7):
`migrations/<YYYYMMDD_HHMMSS>_<name>.sql`, the up statements, a `-- down`
line, the down statements. `migrate up` applies it in one transaction like
any other file. With `model`, `rls make` writes a SeaORM migration file,
`migrations/m<UTC>_<name>.rs`, that runs the same SQL, and prints the
`migrate sync` step that lists it.

## 4. The identity in Postgres

A policy reads who is asking from the transaction:

```sql
USING ("tenant_id" = CAST(NULLIF(current_setting('brocanth.tenant_id', TRUE), '') AS bigint))
```

`brocanth-db` sets it:

```rust
const TENANT: Setting = Setting::of("tenant_id", SqlType::BigInt);   // the policy reads TENANT.get()
let me = DbIdentity::new().role("app_member").setting(&TENANT, 7);
db.transact_as(&me, async |tx| tx.all(&query).await).await?;
```

`Tx::assume(&identity)` runs, inside the transaction:

```sql
SET LOCAL ROLE "app_member";
SELECT set_config($1, $2, true), …;   -- 'brocanth.tenant_id', '7'
```

- **Per transaction.** `SET LOCAL` and `set_config(…, true)` end at
  `COMMIT` or `ROLLBACK`. The pool hands the connection to the next request
  with the login role and no value. A request that does not assume a
  principal gets the login role, and the policy sees `NULL`.
- **The value is a bind parameter.** The role name cannot be one (`SET`
  takes no parameter), so it is a quoted identifier and checked as one.
- **`NULLIF(…, '')`.** After a `set_config` in an earlier transaction on the
  same connection, `current_setting(key, true)` returns `''`, not `NULL`.
  `''::bigint` is an error. `NULLIF` turns both into `NULL`, and
  `tenant_id = NULL` admits no row.
- **Not on `Db`.** A statement on the pool runs in its own implicit
  transaction, so a setting would end with it. The principal exists only on
  a `Tx`.
- **Savepoints.** `assume` on a nested `Tx` ends with the outer
  transaction, but a rolled-back savepoint reverts it.
- **A second `assume` replaces the first.** It does not merge: a key the
  first set and the second does not is set to `''` (read as `NULL`), and
  with no role the transaction returns to the login role
  (`SET LOCAL ROLE NONE`). The `Tx` and its savepoints share the record of
  what was set.
- **One key, one constant.** `DbIdentity::setting(&TENANT, v)` takes the
  `Setting` the policy reads, so the key cannot drift, and refuses a value
  the policy cannot read as its type (`"seven"` for a `bigint`).
  `DbIdentity::set("tenant_id", v)` is the untyped form.
- **The name.** `brocanth_admin::Principal` is the signed-in user of a
  request; `brocanth_db::DbIdentity` is who the database transaction runs
  as. A request maps the first to the second.

The login role needs `GRANT app_member TO app_login` (`Role::member`) for
`SET ROLE`. It needs no privilege on the tables.

### FORCE ROW LEVEL SECURITY

The owner of a table bypasses its policies unless the table is forced.
Migrations often run as the owner, and a small deployment often connects
the application as the owner too. `Table::force()` makes the owner obey the
policies. Use it on every table with tenant data. A superuser and a role
with `BYPASSRLS` still bypass.

## 5. The bridge to `brocanth-policy`

Goal: one rule, written once, drives the application's `WHERE` and the
database policy.

`brocanth-policy` compiles a `Pred<C>` to a `WHERE` fragment with bind
values. Its values come from the subject (`who.user_id`), so the `Pred` for
user 7 holds the literal `7`. A database policy cannot hold `7`: it must
read the setting.

The bridge:

1. **A symbolic bound in `brocanth-policy`.** `Bound::setting("user_id")`, a
   value that `condition()` compiles as a bind of the subject's value and
   that the RLS compiler writes as `setting.get()`. The `policy!` macro
   writes it when a value comes from a field of the subject
   (`author_id == who.user_id else deny`).
2. **A walk in `brocanth-rls`.** `SqlExpr::from_pred::<E>(&pred)` maps
   each public `Pred` variant: `Cmp` to a comparison, `In` to `IN`, `All`
   and `Any` to `AND` and `OR`, `Exists` to `EXISTS`, `Is` to
   `IS NOT DISTINCT FROM`. `Opaque` is refused: its SQL has `?`
   placeholders.
3. **`Policy::from_rule::<P>()`.** It asks the policy for `list`, `create`,
   `update` and `delete` with a symbolic subject, and writes the four
   database policies, as `crud` does.

Step 1 changes `Value` and `Bound` in `brocanth-policy`. Another change to
that crate (its write masks) is in progress, so this task does not build
the bridge. Steps 2 and 3 depend on step 1. It is the next step.

Until then, write the database rule with `SqlExpr` and keep the application
rule. The database rule is the floor: a missing `WHERE` shows nothing it
should not. The application rule stays the one that shapes lists and
masks.

## 6. Rejected alternatives

- **A string DSL or HCL file** (Atlas). A string is parsed at run time, and
  a typo in a column name is found by Postgres in production. Rust code is
  checked by the compiler and reuses sea-query.

  The guarantee is as strong as the names you use. `Table::of(post::Entity)`
  and `SqlExpr::column(post::Column::TenantId)` take a sea-query `Iden`: a
  SeaORM entity and its `Column` enum, or a `#[derive(Iden)]` enum in a
  project without `model`. A typo there does not compile. `Table::named`
  and `SqlExpr::col_named` take a string; they are the escape hatch for a
  table with no entity. A typo in a string is found when the migration
  runs: by `migrate up` in CI or on a laptop, not by the compiler. The
  compiler does not check that a column belongs to the policy's table.
- **Policies inside `brocanth-migrate`'s model scan.** That tool reads model
  source as text with `syn`. A policy expression is Rust that must run to
  produce SQL (sea-query builders), so the scan cannot read it. The schema
  is a function the project's binary calls.
- **The principal as session state (`SET ROLE`, `set_config(…, false)`).**
  It survives the transaction and leaks to the next request on a pooled
  connection. `DISCARD ALL` on release would fix it at the cost of a round
  trip per checkout, and one forgotten reset is a data leak.
- **One Postgres login per user.** It needs a pool per user and a role per
  sign-up. A setting per transaction scales with one pool.
- **`current_user` as the principal.** It works for a few staff roles, not
  for tenants or users. `Setting` covers both; a policy can still say
  `TO "staff"`.
- **Snapshot of the catalog's normalized text.** It would need a database
  at `make` time. See §3.

## 7. What is not done

- The `brocanth-policy` bridge (§5).
- Expression drift in `rls check`. It compares structure only.
- `USAGE` on a schema other than `public`, column-level grants, sequence
  grants (`USAGE` on a `serial` sequence for `INSERT`), and default
  privileges. Grant them in a SQL migration.
- Turning a `Role::new` into a `Role::existing` leaves the snapshot, so the
  next migration drops the role. Remove it from the snapshot by hand
  first if the role must stay.
- Policies on views, and `security_invoker` views.
- Functions in other languages than SQL.
