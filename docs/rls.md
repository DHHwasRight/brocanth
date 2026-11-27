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

let member = Role::new("app_user").member("app_login");      // NOLOGIN, granted to the login
let tenant = Setting::new("tenant_id", SqlType::BigInt);     // brocanth.tenant_id
let posts = Table::of(post::Entity);                        // ENABLE + FORCE

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
| `Table::of(Entity)` / `Table::of_in(s, Entity)`; strings: `Table::named(name)` / `Table::named_in(s, name)` | `ALTER TABLE … ENABLE ROW LEVEL SECURITY` and `… FORCE ROW LEVEL SECURITY` | enabled and forced |
| `.no_force()` | no `FORCE`: the owner bypasses the policies | |
| `.grant(&role, privileges)` | `GRANT SELECT, … ON … TO …` | |
| `.grant_columns(&role, Privilege::Update, ["title"])` | `GRANT UPDATE ("title") ON … TO …`; never with the same privilege on the table | |
| `.sequence("posts_id_seq")` | `GRANT USAGE ON SEQUENCE … TO …` for each grantee with `INSERT` on the table or on columns | no sequence |
| `Policy::new(name, &table)` | `CREATE POLICY` | `AS PERMISSIVE FOR ALL TO PUBLIC` |
| `.restrictive()`, `.command(Command::Select)`, `.to(&role)` | `AS RESTRICTIVE`, `FOR SELECT`, `TO "role"` | |
| `.using(expr)`, `.with_check(expr)` | `USING (…)`, `WITH CHECK (…)` | none |
| `crud(&table, &role, read, modify)` | four policies, as Drizzle's `crudPolicy` | |
| `Policy::from_rules::<P>(&role)` | the table, its grants and up to four policies from a `DbPolicy`'s rules (§5) | |
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
- A grant needs its table and its role. So does a sequence `USAGE` grant
  (`Table::sequence`), one node per grantee.
- A column grant (`Table::grant_columns`) is one node per grantee and
  privilege. It also needs the grantee's table grant: a `REVOKE` on the
  table also revokes the column privileges, so the table grant changes
  first.
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
table, grant, policy, sequence usage, column grant), then by name, so the output is stable. Drops run in
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
policy names, `permissive`, `cmd`, `roles`, table and column grants,
sequence `USAGE`) with
the snapshot. It reports drift. It does not compare expressions, for
reason 1. It also warns about each `.no_force()` table that the
connecting login owns: the owner bypasses the policies of such a table, so
a statement outside `transact_as` sees every row. That is not drift, so it
does not fail the check.

`rls diff` answers a third question with no database: does `schema()`
differ from `migrations/rls.json`? It prints the steps `rls make` would
write and exits non-zero when there are any
(`brocanth_rls::migration::pending`). A rule edit without its migration
moves the application's `WHERE` and leaves the database on the old rule;
CI can run `rls diff` to catch it.

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
| Role | one `ALTER ROLE … [NO]INHERIT`, `[NO]CREATEDB` or `[NO]CREATEROLE` per attribute that differs in the database; membership `GRANT`/`REVOKE` | never |
| Function | body, volatility or `SECURITY DEFINER`: `CREATE OR REPLACE`; `EXECUTE` grants: `REVOKE` the removed, `GRANT` the added | arguments or return type |
| Table | `FORCE`/`NO FORCE` (a table that stays keeps `ENABLE`) | never |
| Grant | `REVOKE` the removed privileges, `GRANT` the added ones | never |
| Sequence `USAGE` | never: the node's key is the table, the sequence and the grantee | a new sequence name is a drop and a create |
| Column grant | `REVOKE` the removed columns, `GRANT` the added ones | never |
| Policy | `ALTER POLICY … TO … USING … WITH CHECK …` | `AS` or `FOR` changed, the table changed, or `USING`/`WITH CHECK` removed (`ALTER POLICY` cannot clear one) |

A recreate spreads to the nodes that depend on it: `DROP FUNCTION` fails
while a policy calls the function, so those policies are recreated too.
A drop spreads the same way. Drops run before alters, so a policy that
named a dropped role, or called a dropped function, is recreated: its drop
runs before the drop of what it needed. A role is never recreated; its
membership of a dropped role ends with the `DROP ROLE`.

Output order: every drop (old graph, reverse topological order), then every
create and alter (new graph, topological order).

The `-- down` part is not the inverse of each step. It is the diff from the
new snapshot to the old one (`down`), so it follows the same rules and the
same order, with one exception: it drops no role (see below). So a removed
membership is always revoked; when the plan drops the member role too, the
`REVOKE` runs only if that role still exists.

A table that leaves `schema()` keeps row-level security in the up part:
its policies and grants are dropped, and `ENABLE` and `FORCE` stay. Fail
closed: disabling it would show every row to a role with a grant made
elsewhere. To turn row-level security off on purpose, write
`ALTER TABLE … DISABLE ROW LEVEL SECURITY` in a SQL migration. The down
part of the migration that added the table still disables it.

Each part starts with `SET LOCAL lock_timeout = '5s'`. `ENABLE` and
`CREATE POLICY` take an `ACCESS EXCLUSIVE` lock; behind a long read they
wait, and every later query waits behind them. With the bound, a deploy
fails fast and can run again.

A rename is a drop and a create. drizzle-kit asks whether a pair is a
rename. Here a policy holds no data, so a drop plus a create loses nothing,
and the migration runs in one transaction, so no request sees the gap:
`brocanth-db`'s migrator runs each file in one, and the SeaORM file says
`use_transaction() -> Some(true)`. A
role rename would lose grants made outside this crate; rename a role by
hand.

### Roles are cluster-wide

`CREATE ROLE` is not per database. Two databases on one server, or a
re-created test database, meet a role that exists. So a role is made in a
`DO $rls$ … $rls$` block:

- Missing: `CREATE ROLE … NOLOGIN`, with only the attributes that differ
  from the `CREATE ROLE` defaults (`NOINHERIT`, `CREATEDB`, `CREATEROLE`).
  The migrate lock covers one database, so another database can create
  the role at the same time: the block catches `duplicate_object` and
  checks the role as it finds it.
- Present: the migration fails, with a `help:` line, when the role has
  `SUPERUSER` or `BYPASSRLS`: either bypasses every policy, so adopting it
  would make the policies a lie. Otherwise each attribute that differs is
  altered, one `ALTER ROLE` each. An attribute that matches is not
  touched, so it needs no privilege.

Removing a role from the schema drops it in the up part only when
`pg_shdepend` shows nothing on the server depends on it; otherwise the
role stays, with a notice. The `-- down` part never drops a role: another
database on the server, or the next deploy, may use it. A role left
behind is adopted by the next up part.

#### The privileges the migration role needs

The migration does not need a superuser:

- `CREATEROLE`, to create a managed role.
- `ADMIN OPTION` on each managed role it did not create, to alter it or
  grant it. On Postgres 16, the creator of a role gets it.
- For `member` (`GRANT role TO login`): from Postgres 16, `ADMIN OPTION`
  on the granted role, which its creator has; before 16, `CREATEROLE`.
- `CREATEDB` only to set or clear `CREATEDB` on a role.
- Ownership of each table (or membership of its owner), for `ENABLE`,
  `FORCE` and `CREATE POLICY`, and `CREATE` on the schema of a function.

A refused `ALTER ROLE` fails with a `help:` line that names these.

`BYPASSRLS` and `SUPERUSER` are not offered. A superuser bypasses every
policy, `FORCE` included. Do not connect the application as a superuser.

#### `SECURITY DEFINER`

`Function::security_definer(&["app"])` writes
`SET search_path = "app", pg_temp`: `pg_temp` last, so a caller's
temporary table cannot stand in for a table the body reads. `EXECUTE` is
revoked from `PUBLIC` and granted to the roles `grant_execute` names. A
policy that calls the function for a role without the grant is refused.

### Where the file goes

The `db` module's SQL migrations (`docs/migrations.md` §7):
`migrations/<YYYYMMDD_HHMMSS>_<name>.sql`, the up statements, a `-- down`
line, the down statements. `migrate up` applies it in one transaction like
any other file. The SeaORM migrator also runs each migration in one
transaction on Postgres; the file `rls make` writes asks for it
explicitly. With `model`, `rls make` writes a SeaORM migration file,
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
let me = DbIdentity::new().role("app_user").setting(&TENANT, 7);
db.transact_as(&me, async |tx| tx.all(&query).await).await?;
```

`Tx::assume(&identity)` runs, inside the transaction:

```sql
SET LOCAL ROLE "app_user";
SELECT set_config($1, $2, true), …;   -- 'brocanth.tenant_id', '7'
```

- **Per transaction.** `SET LOCAL` and `set_config(…, true)` end at
  `COMMIT` or `ROLLBACK`. The pool hands the connection to the next request
  with the login role and no value. A request that does not assume an
  identity gets the login role, and the policy sees `NULL`.
- **The value is a bind parameter.** The role name cannot be one (`SET`
  takes no parameter), so it is a quoted identifier and checked as one.
- **`NULLIF(…, '')`.** After a `set_config` in an earlier transaction on the
  same connection, `current_setting(key, true)` returns `''`, not `NULL`.
  `''::bigint` is an error. `NULLIF` turns both into `NULL`, and
  `tenant_id = NULL` admits no row. That holds for `eq` and the other
  comparisons. `is` (`IS NOT DISTINCT FROM`) would admit every row whose
  column is `NULL`, so a schema refuses `is` on a setting.
- **Not on `Db`.** A statement on the pool runs in its own implicit
  transaction, so a setting would end with it. The principal exists only on
  a `Tx`.
- **Savepoints.** `assume` on a nested `Tx` ends with the outer
  transaction, but a rolled-back savepoint reverts it.
- **A second `assume` replaces the first.** It does not merge: a key the
  first set and the second does not is set to `''` (the policies of
  `brocanth-rls` read it as `NULL` through `nullif(.., '')`; a policy
  written by hand must do the same), and
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

### How a request sets its identity

docs/policy-rls-bridge.md, decision 6. With a `brocanth-policy` subject,
the identity is built in one place:

```rust
let me = brocanth_rls::identity(&app_user(), &visitor);   // DbIdentity
```

`identity` sets the role and **every** key of `Subject::ATTRS`, one
`brocanth.<key>` per attribute. An absent value (`None`) sets `''`, which
`NULLIF` reads as `NULL`, so "absent" means "anonymous" in the application
and in the database. A present value goes through the typed `Setting` of
its kind (`bigint`, `integer`, `boolean`, `text`, `uuid`, `timestamptz`).
It is written as Postgres casts it back: a uuid hyphenated, a timestamp as
ISO 8601 in UTC with microseconds. A value the type refuses fails at
`assume`, with a `help:` line.

In a generated project with `rls` and `policy`, `src/plugins/rls/mod.rs`
has:

- `app_user()`: the one role every request runs as (decision 7).
  `schema()` creates it. The login needs `.member(<login>)` unless it is a
  superuser.
- `schema()`, with `.policies(Policy::from_rules::<PostPolicy>(&app_user()))`
  (§5). The subject's attributes come from `#[derive(Subject)]` on
  `Visitor`, in `src/plugins/policy/mod.rs`.
- `RowSecurity`: a plugin that puts `Identity(identity(&app_user(),
  &visitor))` on each request's typed context. The visitor is the one a
  request hook put in the request state; without one, the request is
  anonymous. `configure` builds the anonymous identity once, so a bad role
  name stops the start.
- `identity(ctx)`: the request's `DbIdentity`, for `transact_as`.

A handler then runs its work as that identity:

```rust
let me = crate::plugins::rls::identity(ctx).expect("dispatched");
// sqlx
let rows = ctx.db().transact_as(me, async |tx| tx.all(&q).await).await?;
// SeaORM
let orm = ctx.provided::<brocanth_model::Orm, _>().expect("dispatched");
let posts = orm.transact_as(me, async |txn| post::Entity::find().all(txn).await).await?;
```

`Identity` is a newtype: `ContextItem` is the core's trait and `DbIdentity`
is `brocanth-db`'s type, so the generated project cannot implement one for
the other (the reason `brocanth_model::Orm` is a newtype too).

The records API and the backoffice carry the identity too (decision 13):

- `brocanth-resource`: `ResourceData::session(&identity)` opens one session
  per operation, and `flow::update_as(resource, &identity, …)` runs the
  admission read, the `Verified` write and the re-read in it.
  `PageSubject::identity` reads the identity off the request.
- `brocanth-admin-model`: `Models::new(db).as_identity(staff)` runs each
  request in a transaction as `staff(principal)`. `docs/backoffice.md` §2a.

The login role needs `GRANT app_user TO app_login` (`Role::member`) for
`SET ROLE`. It needs no privilege on the tables.

### FORCE ROW LEVEL SECURITY

The owner of a table bypasses its policies unless the table is forced.
Migrations often run as the owner, and a small deployment often connects
the application as the owner too. So a table is forced by default: the
owner obeys the policies. `Table::no_force()` is the exception, for a table
the owner must read whole. A superuser and a role with `BYPASSRLS` still
bypass. (Decision 2026-11-12: the default was "not forced", and an
application that connected as the owner saw every row.)

## 5. The bridge to `brocanth-policy`

One rule, written once in `policy!`, drives the application's `WHERE` and
the database policy. The design, its decisions and their reasons are in
[`docs/policy-rls-bridge.md`](policy-rls-bridge.md). This section says how
to use it.

Mark the policy `#[db]`, derive `Subject` on its subject, and give the
policy to the schema:

```rust
Schema::new()
    .role(app_user())
    .policies(
        Policy::from_rules::<PostPolicy>(&app_user())
            .sequence("posts_id_seq")
            // An `#[app_only]` rule needs its database floor by hand.
            .manual(Command::Update, SqlExpr::column(post::Column::Status).eq(SqlExpr::val("draft"))),
    )
```

- `Policy::from_rules::<P>(&role)` writes `<table>_<role>_select`
  (`list OR view`), `_insert` (`create`), `_update` (`USING update WITH
  CHECK update_check`) and `_delete`. A rule that admits no row writes no
  policy and no grant. The table is forced, and the role gets each command
  that has a policy. `.sequence(…)` grants `USAGE` on the table's serial
  sequence when the role may insert.
- `.inserter(role)` (bridge decision 17): a second role that inserts any
  row and does nothing else, a system writer. It gets `INSERT`, `USAGE`
  on the sequence, and `<table>_<role>_insert`, `WITH CHECK (TRUE)`. No
  `SELECT`, so `RETURNING` fails for it. `Schema::policies` adds the role
  to the schema. With `create: never`, the main role cannot insert.
- Link tables (bridge decision 15). The role gets `SELECT` on each link
  table an `exists` reads, because the subquery runs as the role. When
  `schema()` adds that table too, the grant joins its grants; else the
  snapshot keeps it in `link_grants`, and the table gets no row-level
  security from it. `rls diff`, `rls make` and `rls check` see it, and the
  next migration revokes it when no rule reads the table. If the link
  table has row-level security, its own policies filter the subquery: with
  no `SELECT` policy there for the role, the `exists` rule denies every
  row. Give it a policy that shows the role the rows the rule needs.
- Write masks (bridge decision 9). A `writable` mask with no guard
  (`writable: only [title]`) grants `UPDATE ("title")` instead of `UPDATE`
  on the table; `writable_on_create`, or `writable` when there is none,
  does the same for `INSERT`. Never both. `except [id]` grants every other
  column that the entity declares when `rls make` runs, so a new column
  shows in `rls diff`. `all`, or no mask, keeps the table grant. `none`
  writes no policy and no grant for the command. A mask with a guard or a
  `{ … }` block keeps the table grant: only the application checks it, and
  `rls make` prints one `warning:` line per such table
  (`Schema::warnings()`). The table `SELECT` grant stays, so an `UPDATE …
  WHERE id = $1 AND status = 'draft'` can read its `WHERE` columns. A write
  path must write only the columns it changes: an `UPDATE` that sets a
  column outside the mask, even to its old value, fails with `permission
  denied for table`.
- `SqlExpr::from_rule(&rule, &table, Visitor::ATTRS)` writes one rule, by
  the translation table of the bridge doc §1. Each subject attribute is a
  `Setting` of its kind, read as `NULL` when it is empty. `else deny` is
  FALSE, not NULL, when the setting is missing, and a guard chain is
  boolean, not a `CASE` over the arms. `Policy::from_rules` adds a key gate to an
  `always` arm of a guard chain, `"id" >= CASE WHEN <guards> THEN <least
  id> END`, so the other arms keep their index (bridge doc, decision 14).
- `Schema::snapshot`, and so `rls make`, refuses with a `help:` line a
  rule the database cannot check: an app-only rule (`#[app_only]`) or a
  shape decision 3 refuses. `.manual(command, expr)` replaces that
  command's expression.
- `rls diff` fails when `schema()` differs from `migrations/rls.json`. It
  reads no database. The generated CI runs it, so a rule edit cannot ship
  without its migration.

`brocanth create` writes this for a project with `rls` and `policy`: the
policy scaffold's `PostPolicy` is `#[db]`, with an `update_check` that lets
an author publish a draft, and `src/plugins/rls/mod.rs` reads it by path.
The example's `db { … }` clause runs the agreement test in `cargo test`, on
`[db] url` (`agreement_url()`), and skips when nothing answers there.
The first `rls make` alters `posts`, so the table must exist before
`migrate up`.

A hand-written `impl Policy` has no rule tree: write its database rule
with `SqlExpr` and `crud`. The database rule is the floor: a missing
`WHERE` shows nothing it should not.

### The agreement test

`brocanth_rls::agreement` (feature `agreement`) checks that a
`DbPolicy`'s database policies admit the rows its `condition()` admits
(bridge decision 10). `policy!`'s `db { … }` clause calls `run`; a test
can call `check::<P>(&url, &subjects, cases)` directly.

Per case, it creates a schema with the entity's table (from
`TypedColumn::sql_type`), each link table an `exists` reads (no grant of
its own: the `from_rules` SQL grants it), and the SQL of
`Policy::from_rules`. It inserts generated rows: the values of
`brocanth_policy::agreement::pool`, with `NULL`s and near-misses. Then,
for each subject:

| Command | Application | Database (`transact_as(identity(subject))`) |
|---|---|---|
| `SELECT` | `SELECT id … WHERE <list OR view>` as the URL's user | `SELECT id …`, no `WHERE` |
| `INSERT` | `create.eval` on each candidate row | the `INSERT`, in a savepoint |
| `UPDATE` | `update` on the stored row, `update_check` on the new row | the `UPDATE`, in a savepoint: done, no row, or refused |
| `DELETE` | `delete` on each row | the `DELETE`, in a savepoint |
| `UPDATE`, no `update_check` | `update` admits the stored row, and the subject's `writable` mask permits the column | an `UPDATE` of one column that `update` reads, to each sampled value: refused fails the test |

An `UPDATE` sets only the columns of an unguarded `writable` mask. An
`INSERT` writes every column, so the test grants `INSERT` on the whole
table: it compares rows, not column grants. An `#[app_only]` command, and
a write command whose mask permits no column, is left out.

It also unsets each setting in turn for one subject and compares `SELECT`
again (a missing attribute denies, as in the application), and runs one
`SELECT` with no identity, which must fail with `permission denied`. It
fails when `update`, `delete` or `update_check` reach a row outside
`list OR view`: Postgres hides such a row from the statement, or refuses
the new row.

The last row of the table finds a forgotten `update_check` (bridge
decision 16). Without it, the database checks the new row with `update`,
and the application does not check the new row at all. So an author who
may set `status` on a draft can publish it in the application, and the
database refuses it. The test fails with ``help: write `update_check` …,
or remove `status` from `writable` ``. The mask is the subject's own
(`DbPolicy::writable_of`, which `policy!` writes), so a guarded mask
counts.

The URL's user must be a superuser: the test creates two roles per run
and reads around the policies as the application's side. `tests/agreement.rs`
runs it over fixed rules that cover each row of the translation table,
guard chains, `exists`, `update_check` (and a forgotten one), `else
null`, and `uuid` and `timestamptz` columns. `tests/bridge_postgres.rs`
checks with `EXPLAIN` that a comparison with a setting keeps the column's
index, and that a guard chain with an `always` arm keeps it through the
key gate. `tests/agreement_keys.rs` runs the harness over the gate for
each key type.

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

- Expression drift in `rls check`. It compares structure only.
- `USAGE` on a schema other than `public`, and default privileges. Grant
  them in a SQL migration.
- Turning a `Role::new` into a `Role::existing` leaves the snapshot, so the
  next migration drops the role. Remove it from the snapshot by hand
  first if the role must stay.
- Policies on views, and `security_invoker` views.
- Functions in other languages than SQL.
- The admin's data source runs as the staff member's identity only with
  `as_identity` (§4, "How a request sets its identity"). Without it, it
  queries the `Orm` as the login role: on a `FORCE` table it sees no row.
- `rls check` does not compare a function's `EXECUTE` grants.
