# One rule, two enforcers: the policy-RLS bridge

Written 12 November 2026. Status: **accepted 17 November 2026; phases 1
to 7 built** (18 November 2026): the rule tree and `#[derive(Subject)]`,
`policy!` `#[db]`, `Policy::from_rules`, the request identity on the
resource and admin paths, the agreement test (`brocanth_rls::agreement`,
the `db { … }` clause, the `EXPLAIN` check; notes under decision 10), the
scaffold with `rls diff` in CI, and column grants for write masks with no
guard (decision 9). Decision 14 (21 November 2026): a guard chain keeps
the column's index. Decision 15 (21 November 2026): link-table grants.
Decision 16: the forgotten `update_check` check. PLAN.md §13 ADR 11
records the decision.
Postgres only. §7 records the owner's answers.

Owner decision: one rule drives the application's `WHERE` (`brocanth-policy`
`condition()` for lists, `Verified` for writes) and the database policy
(`brocanth-rls`). The author writes the rule once.

This doc uses the API that `feat/rls` landed on main (54db8067):
`DbIdentity` (was `Principal`), typed `&Setting` setters, `is()` refused on
a setting, typed SeaORM columns, `Orm::transact_as`. It replaces the sketch
in `docs/rls.md` §5.

## 0. The problem in one example

```rust
pub policy PostPolicy(who: &Visitor) for Post {
    list {
        who.banned => never,
        who.admin => always,
        status == "published" || author_id == who.user_id else deny,
    }
}
```

The `who.admin` guard is a Rust `if`. A symbolic subject cannot follow an
`if`, so rls.md §5's plan fails on the first guard. And `who.user_id` folds
into the `Pred` as a literal `7`, where the database must read
`brocanth.user_id`. The bridge needs the rule **before** Rust evaluates it.
Only the `policy!` macro has it, as syntax.

## 1. What translates, and what does not

### Decision 1: the bridge is a `policy!` feature. A hand-written `impl Policy` gets no database policy.

A policy opts in with `#[db]` on the `policy` item. The macro then emits a
symbolic rule set, `Rules<E>`, next to the `impl Policy`. `brocanth-rls`
reads `Rules<E>`, never the trait.

Reason: no tool can read the branches of a hand-written `impl Policy`
without running them with a concrete subject. In the macro, the rule is
still data.

### Decision 2: with `#[db]`, `impl Policy` is the symbolic rule resolved with the subject's values.

The macro emits `fn rules() -> Rules<Post>` and implements
`Policy::list(who)` as `rules().list.resolve(&who.attributes())`. `resolve`
walks the same tree, takes each guard's branch and folds each attribute into
a `Bound`. So `condition()` and the RLS expression come from one tree.

Reason: two expansions can drift through a macro bug. One tree with two
readers drifts only where the readers differ, and §4 tests that. Without
`#[db]`, the expansion does not change.

### The translation table

`s(k)` is the setting read: `CAST(NULLIF(current_setting('brocanth.k', TRUE), '') AS <type>)`.

| In the rule | In the database policy | Notes |
|---|---|---|
| `always`, `never` | `TRUE`, `FALSE` | |
| `col == lit` (and `!=`, `<`, …) | `"col" = 'lit'` | literal, written by sea-query |
| `col == who.x else deny` | `(s(x) IS NOT NULL AND "col" = s(x))` | FALSE, not NULL, when absent; see §4 |
| `col == who.x else allow` | refused with `#[db]` | decision 3 |
| `col == who.x else null` | `"col" = s(x)` | three-valued, as the app |
| `col == who.x` (non-`Option` field) | `(s(x) IS NOT NULL AND "col" = s(x))` | a NULL setting fails closed |
| `col is lit`, `col is null` | `IS NOT DISTINCT FROM 'lit'`, `IS NULL` | literal only |
| `col in [lits]`, `not in` | `IN (…)`, `NOT IN (…)` | literal only |
| `&&`, `\|\|`, `!` | `AND`, `OR`, `NOT` | |
| `exists T(…)` (membership, new syntax) | `EXISTS (SELECT 1 FROM "t" WHERE …)` | filter values may be `who.x`; see below |
| guard `who.f`, `!who.f`, `who.f == lit`, `&&`/`\|\|` of them | boolean over `s(f)` | see below |

A guard chain `g1 => r1, g2 => r2, f` becomes:

```sql
s(g1) IS NOT NULL AND s(g2) IS NOT NULL AND (
     (g1 AND r1)
  OR (NOT g1 AND g2 AND r2)
  OR (NOT g1 AND NOT g2 AND f))
```

In `Policy::from_rules`, an arm whose term is `always` also gets the key
gate of decision 14: `g1 => always` is `(g1 AND "id" >= CASE WHEN g1 THEN
<least id> END)`. `SqlExpr::from_rule` writes the bare form: it does not
know the key.

Membership needs a syntax: today `Pred::Exists` enters a `policy!` only
through a `{ }` block, which decision 3 refuses. Phase 2 adds

```rust
list { exists Member(post_id == id, user_id == who.user_id else deny) }
```

where the left side of each `==` is a column of `Member` and the right side
is a column of the outer entity, a literal, or `who.<field>`. It builds the
same `Pred::Exists` as the hand-written form.

`!exists` (phase 2): `!exists T(…)` is `Exists { negated: true }`, one
term. An absent `else deny` value in its filter makes it `never`, in
`resolve` as for a plain `exists`. So the database writes
`s(x) IS NOT NULL AND NOT EXISTS (…)` for each such `x`, not
`NOT EXISTS (… s(x) …)`, which is TRUE when `x` is missing. Reason:
fail-closed, and the two readers agree. `policy!` refuses `!` over a larger
predicate that holds such an `exists`, so `Term::Not(Term::Exists(…))`
never comes from the macro.

A term with `never` drops out. A NULL guard setting makes the rule FALSE.
Reason: a Rust `bool` has no third value; without the prefix, a missing
`brocanth.banned` reads as "not banned". The form is boolean, not `CASE`,
so the planner sees plain `AND`/`OR`. A comparison with a setting keeps
the column's index (phase 5), and decision 14 keeps it in a guard chain.

### Decision 14: a guard arm that reads no column gets a key gate, so a guard chain keeps the index.

`Policy::from_rules` writes an `always` arm of a guard chain as

```sql
(cond AND "id" >= CASE WHEN cond THEN <least value of the key type> END)
```

where `cond` is the arm's guards (`NOT g1 AND g2`) and `id` is the
entity's `#[key]`. A primary key is never NULL, so the gate is TRUE for
every row when `cond` is TRUE and NULL otherwise: the same truth as
`cond`. But it is an index condition. The planner can put the arm in a
`BitmapOr` with the other arms, and when `cond` is not TRUE the key scan
returns no row at once. The gate goes only on an arm whose term is
`always`, and only when the rule has two or more arms left after `never`
arms drop out. The least values: `smallint`, `integer` and `bigint` minima,
`-Infinity` for `real` and `double precision` (`NaN` sorts above it), `''`
for `text` and `bytea`, the nil `uuid`, and `-infinity` for `timestamptz`.
A key of another type gets no gate, and the rule keeps the bare form.

Reason: Postgres plans one tree for every subject. An `OR` uses an index
only when each branch has an index condition (`BitmapOr`), and `g AND
TRUE` has none, so one `always` arm made every subject scan the table. The
gate gives that branch an index condition whose value the executor reads
from the setting, so the plan is right for each subject, also as a cached
generic plan.

Measured on Postgres 16, 10 000 rows, an index on `author_id` and on
`team_id`, rule `who.admin => always, author_id == who.user_id else deny`
(plan node for a user / for an admin):

| Shape | User | Admin |
|---|---|---|
| bare boolean form (before) | Seq Scan | Seq Scan |
| (a) one `PERMISSIVE` policy per arm | Seq Scan | Seq Scan |
| `RESTRICTIVE` policy for the `IS NOT NULL` prefix, body permissive | Seq Scan | Seq Scan |
| (b) setting read in a `STABLE` or `STABLE LEAKPROOF` SQL function | Seq Scan | Seq Scan |
| (b) setting read in an `IMMUTABLE` SQL function | Bitmap Index Scan (author) | Seq Scan |
| (c) key gate on the `always` arm | BitmapOr: pkey (0 rows), author (10 rows) | Seq Scan |
| (c) key gate, generic plan (`force_generic_plan`) | BitmapOr, 10 rows | BitmapOr, 10 000 rows |
| bare form, both arms compare an indexed column | BitmapOr | BitmapOr |

Rejected:

- **(a) one policy per arm.** Postgres ORs the permissive policies of a
  command into one expression before it plans, so the plan does not
  change, and the split renames every policy, which costs a drop and a
  create in each project.
- **(b) a function the planner folds.** Only `IMMUTABLE` folds, and it
  folds the setting into the plan: a statement prepared as a user and run
  again as an admin (`force_generic_plan`) returned 10 rows, not 10 000.
  The reverse leaks rows. sqlx caches prepared statements per pooled
  connection, so a plan built for one request runs for the next.
- **The gate on every arm.** For an arm that compares an indexed column,
  the planner then read the whole key index for the admin, not the
  `team_id` index. An arm that compares a column keeps its own index; index
  the columns that your guarded arms compare.

`tests/bridge_postgres.rs` checks the plans (50 000 rows): the bare form
is a `Seq Scan` for a user; after `rls diff` moves it (one `ALTER
POLICY`, names do not change), a user's plan is a `BitmapOr` of the key
and the author index, and staff scan the table. `tests/agreement_keys.rs`
runs the agreement harness over a guard chain for each key type the gate
knows.

### Decision 3: these shapes are refused, at compile time with `#[db]`, or at `rls make`.

| Shape | Refused where | `help:` line |
|---|---|---|
| `{ rust }` block in a rule or a guard | macro | `help: write the rule in the policy! syntax, or mark it #[app_only("reason")]` |
| a guard that is not `who.<field>`, `!`, `== lit`, `&&`, `\|\|` | macro | `help: add the value to the subject as a #[attr] field and test that` |
| a value that uses `who` and is not exactly `who.<field>` | macro | `help: compute the value when you build the subject, and store it in a field` |
| `col is who.x`, `col is not who.x` | macro | `help: use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting` |
| `col in who.xs` (a list attribute) | macro | `help: a setting holds one value; use a membership table and exists` |
| `col == who.x else allow` | macro | `help: put the fact in a bool #[attr] field and guard on it` |
| `Pred::Opaque` | macro (it is a `{ }` block) | as the first row |
| a `#[db]` policy whose subject field has no `#[attr]` | compiler (the accessor does not exist) | the derive's error |
| an `#[app_only]` rule on a command | `rls make` | `help: write a SqlExpr for <table> <command> in src/plugins/rls/mod.rs with .manual(...)` |

Reason for the `else allow` refusal: it compiles to `(s(x) IS NULL OR …)`.
A key that is missing from the identity reads as NULL (a second `assume`
sets a dropped key to `''`), so a missing key opens the table.
`check_identity` refuses unknown keys but does not require every key. A
guard on a bool attribute fails closed instead (the `IS NOT NULL` prefix).

Reason for the `is` refusal: `NULL IS NOT DISTINCT FROM NULL` is TRUE, so
a request with no `user_id` matches every row with no author. `feat/rls`
refuses the same on `SqlExpr`.

### Decision 4: an app-only rule never becomes open or closed by default.

`#[app_only("reason")]` on one rule lets the app keep a Rust-only check.
In `rules()`, that rule is `Rule::app_only(reason)`: `Rule::app_only` is
`Some(reason)`, with no arms and a `never` fallback. A reader tests
`rule.app_only` first and never reads the arms of such a rule (phase 2).
`rls make` then refuses until `schema()` gives that table and command a
hand-written `SqlExpr` (`Policy::from_rules::<P>(&role).manual(Command::Update, expr)`).

Reason: both silent choices are wrong. No policy denies the command, so
the app breaks in production, not in review. `TRUE` removes the floor.

## 2. Subject attributes and settings

### Decision 5: one `Setting` per subject attribute, typed by the field.

```rust
#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr] pub user_id: Option<i64>,   // brocanth.user_id  bigint
    #[attr] pub tenant_id: i64,         // brocanth.tenant_id bigint
    #[attr] pub admin: bool,            // brocanth.admin    boolean
    #[attr] pub banned: bool,
    pub display_name: String,           // not an attribute: no rule reads it
}
```

The derive emits `Visitor::ATTRS: &[Attr]` (key, kind, optional) and
`fn attributes(&self) -> Attributes`. The kinds are the `Value` kinds:
`i64`/`i32` -> `bigint`/`integer`, `bool` -> `boolean`, `String` -> `text`,
`Uuid` -> `uuid`, `chrono::DateTime<Utc>` -> `timestamptz`. `brocanth-rls` turns
each `Attr` into a typed `Setting`.

Phase 1 adds `Uuid` and `Timestamptz` as `typed::Kind`s and `Value`
variants. Today a policy entity cannot have a uuid column at all
(`ColumnType` covers integers, floats, `bool`, `String`, `Vec<u8>`).
Reason against `text` and a cast: the cast lands on the column, so
Postgres cannot use the column's index, and phase 5 checks that it does.

Reasons: the key comes from the field name, so a rename changes both sides
at the next build. `Attr` is plain data, so `brocanth-policy` stays free of
drivers, and `brocanth-rls` depends on it, not the reverse. Only `#[attr]`
fields reach the database, so an email does not leak into settings.

### Decision 6: the identity is built in one place, and every key is always set.

`brocanth_rls::identity(&role, &visitor) -> DbIdentity` sets the role and
**every** key in `ATTRS`. `None` sets `''`, which `NULLIF` reads as NULL.

Reason: "absent" has one meaning, "anonymous", in both places.
`Schema::check_identity` (feat/rls) refuses a key no policy reads.

### How a request sets it

The auth hook builds the subject. The `rls` plugin puts the identity on
the typed context (module-config.md §9 decision 3):

```rust
// src/plugins/rls/mod.rs (generated)
fn provide_for_request(&self, req: &RequestState, cx: &mut ContextBuilder) {
    let who = crate::plugins::policy::visitor(req);           // the app's subject
    cx.put(brocanth_rls::identity(&APP_USER, &who));          // DbIdentity
}
```

A handler then runs its work as that identity:

```rust
let me = ctx.provided::<DbIdentity, _>()?;
// sqlx path
let rows = ctx.db().transact_as(me, async |tx| tx.all(&q).await).await?;
// SeaORM path (target API)
let posts = ctx.orm().transact_as(me, async |tx| Post::find().all(tx).await).await?;
```

Both call `Tx::assume` (`SET LOCAL ROLE`, `set_config(k, v, true)`).
Nothing goes on the pool.

Decision 7: one role, `app_user`, for every request under a policy.
Guards on settings express "admin". Reason: a role per kind of user needs a
grant and a migration per kind; a setting costs nothing. A `staff` role by
hand stays possible.

`Verified` writes run in the same `Tx`: Postgres adds the RLS policy to the
app's `UPDATE … WHERE id = ? AND <condition>`. Two checks, one statement.

### Decision 13: resource and admin paths carry the identity.

`ResourceData` today is `&self` and `'static`, with no `Tx` and no
identity, and the admin data source has none either. On a forced table
they would see no rows. Phase 4 gives `ResourceData` the request's
`DbIdentity` and runs each read and write in `transact_as`. The backoffice
builds the identity of the staff member: its per-tab RBAC
(module-capabilities.md §5.2) still folds into `condition()`, and the
database checks the same rule.

Reason: without it, the first `#[db]` table breaks the backoffice, and the
"two checks, one statement" claim above has no call site.

Built in phase 4 (17 November 2026), with three changes to the sketches
above:

- `APP_USER` is a function, `app_user() -> Role`: `Role` holds a `String`,
  so it cannot be a `const`.
- The context item is `Identity(DbIdentity)`, a newtype of the generated
  project, read with `crate::plugins::rls::identity(ctx)`. Reason: the
  orphan rule. `ContextItem` is the core's trait and `DbIdentity` is
  `brocanth-db`'s type.
- `brocanth-resource` links no driver, so it cannot name `DbIdentity`.
  `ResourceData` names its own `type Identity` (`()` for a source with
  none) and opens a `Session` per operation; `flow::*_as` runs as an
  identity, and `flow::*` as `Identity::default()`. The admin already had
  a per-request source (`AdminData::for_request`), so `admin_source!` gained
  `as_identity` and the admin gained `AdminData::check`, run at build.

Phase 6 removed the hand-written `impl Subject for Visitor` from the
generated `src/plugins/rls/mod.rs`: the policy scaffold derives `Subject`.

## 3. Commands

`brocanth_rls::Policy::from_rules::<P>(&role)` writes up to four policies,
named `<table>_<role>_<command>`, like `crud`.

| `Policy` method | Database policy | Reason |
|---|---|---|
| `list` OR `view` | `FOR SELECT USING (list OR view)` | Postgres has one read command. The database floor is the union. The app narrows a list with `list`. `view` defaults to `list`, so the union is `list` for most policies. |
| `create` | `FOR INSERT WITH CHECK (create)` | both check the proposed row; grants `USAGE` on the table's serial sequence |
| `update` | `FOR UPDATE USING (update) WITH CHECK (update_check)` | see decision 8 |
| `delete` | `FOR DELETE USING (delete)` | |
| a rule that is `never` for every guard | no policy, no grant | RLS denies what no policy allows. No grant gives a clear `permission denied`. |

`from_rules` also grants the commands that have a policy, and forces RLS.
It grants `SELECT` on each link table an `exists` reads (decision 15).

### Decision 8: `UPDATE … WITH CHECK` uses `update_check`, which defaults to the `update` rule.

The app's `update` checks the stored row only (policy.rs: "decides on the
row as stored"). The `writable` mask is what stops an author from moving a
post to somebody else. The database adds `WITH CHECK (update)` on the new
row too.

Reason: the extra strictness covers the hole the mask covers, so the
database refuses an owner change when a write path forgets the mask. Cost:
`update: status == "draft"` with `status` writable lets the app publish a
draft that the database refuses. So `update_check: <pred>` ships in phase
2, not later (optional; `always` = stored row only), and the scaffold's
example policy is a status change (`draft` to `published`) that uses it.
§4's test finds every other case.

Postgres also applies the `SELECT` policy to an `UPDATE`/`DELETE` that
reads a column, so a row you may update but not see stays unchanged. §4's
test fails on such a row; the docs say: keep `update` and `delete` inside
`list`.

### Decision 15: `from_rules` grants `SELECT` on each link table an `exists` reads.

The `EXISTS` subquery of a policy runs as the request's role, so the role
needs `SELECT` on the link table. `Policy::from_rules` takes the table's
name from `Exists::table`, for each command that has a policy, and
`Schema::policies` adds the grant. In the snapshot:

- When `schema()` adds the link table too (its own `Table`, or its own
  `from_rules`), the `SELECT` joins that table's grants. A column `SELECT`
  for the role there stays as written: a table grant would replace the
  developer's choice of columns.
- Else it goes into `link_grants`, an optional field, so an older
  `rls.json` reads the same: `GRANT SELECT ON "t" TO "role"`, and no
  row-level security on `t`.

`rls diff`, `rls make` and `catalog::drift` see it like any grant. On a
link table the schema does not add, `drift` checks only that `SELECT`, not
the table's other privileges. When the rule stops reading the table, the
next migration revokes it. A link to the policy's own table, a command with
no policy, and a `.manual(…)` expression add no grant: grant what a
hand-written expression reads in `schema()`.

Reasons: before, the developer added the grant by hand, and a forgotten
grant made every `exists` rule fail with `permission denied for table`. The
rule names the table, so the grant follows from it, as the sequence `USAGE`
follows from `INSERT`. The grant does not turn on row-level security on the
link table: with no policy there, every `exists` would deny.

**A link table with row-level security filters the subquery.** Postgres
applies the link table's own policies to the `EXISTS` subquery, for the
request's role. With row-level security on the link table and no `SELECT`
policy there for the role, the subquery sees no row, and the `exists` rule
denies every row. Give the link table a `SELECT` policy that shows the role
the rows the rule needs: for membership, the reader's own rows.
`tests/bridge_postgres.rs` proves both, and that `drift` reports a revoked
link grant. The agreement harness creates each link table with no grant of
its own, so it proves that `from_rules` grants it.

### Decision 9: write masks stay in the app in the first version.

RLS filters rows, not columns. The only database tool for columns is a
column grant: `GRANT UPDATE (title) ON posts TO app_user`. A grant is per
role, and a mask is per subject (`who.admin => except [id]`). One role
(decision 7) cannot carry two masks.

So `writable` and `writable_on_create` stay app-only. `brocanth-resource`
and the admin check them before any row is read, as today. Phase 7 adds
column grants for a mask with **no guard** (the same mask for every subject
of the role): the grant is then exact. A guarded mask stays app-only, and
`rls make` prints one line per such table, so the gap is visible.

Phase 7, as built. `#[db]` puts each mask in `Rules` as data:
`MaskRule::Fixed(FieldMask)` when the mask has no guard and no `{ … }`
block, else `MaskRule::PerSubject`. `Policy::from_rules` then grants:

| Mask | `INSERT` / `UPDATE` grant |
|---|---|
| none written | on the table, as before |
| `all` | on the table |
| `only [..]`, `except [..]` | on the columns it permits, never also on the table |
| `none` | no grant and no policy for the command |
| guard or block | on the table, and one `warning:` line per table in `rls make` |

`INSERT` reads `writable_on_create`, or `writable` when there is none, as
the trait does. The table `SELECT` grant stays, so `UPDATE … WHERE id = $1
AND <condition>` reads its columns; a Postgres test proves it, and that a
column outside the mask fails with `permission denied`. The snapshot holds
the column grants in an optional `column_grants` field, so an older
`rls.json` reads the same; the diff revokes and grants columns, and
`rls check` compares them.

Reason for "none written: table grant": the trait's default mask denies
every column, so the exact grant would be no `UPDATE` at all. That would
change every existing `#[db]` policy that writes no mask. A project that
wants the grant writes the mask.

## 4. NULL, fail-closed, and the agreement test

### The layers that fail closed

1. **No identity, no grant.** Without `transact_as`, the query runs as the
   login role, which has no table privilege: `permission denied`. This
   holds only when the login does not own the table. A small deployment
   that connects as the owner sees every row of a `.no_force()` table.
   `rls check` reports each `no_force` table that the login owns.
2. **A missing non-`Option` attribute denies** (`s(x) IS NOT NULL AND …`).
3. **`else deny` is FALSE, not NULL.** The app folds `or_deny(None)` to
   `Never`, and `negate(Never)` is `Always`. `NOT (col = NULL)` is NULL, a
   deny: they would disagree. `(s IS NOT NULL AND col = s)` is FALSE, and
   `NOT FALSE` is TRUE, as in the app. Faithful beats "more closed".
4. **`else allow` is refused** with `#[db]` (decision 3).

### Decision 10: an agreement test, on Postgres, per `#[db]` policy.

The `eval { … }` clause (docs/policy.md) proves `eval` agrees with
`condition()`. A new `db { … }` clause proves `condition()` agrees with RLS:

```rust
db {
    subjects: [Visitor::anonymous(), Visitor::user(1), Visitor::admin(), Visitor::banned(1)],
    cases: 64,
}
```

In this repo it reads `BROCANTH_TEST_DATABASE_URL`. In an app it is on by
default, with no flag: it reads the app's `[db] url` through the settings
layers, as the generated `rls check` does, and prints `!! SKIPPED` when no
URL is set. The generated CI already starts Postgres for a project with
`db`. Per case it:

1. creates a schema, the table from the entity's column types, the role and
   the policies from `rules()` (the same SQL `rls make` writes);
2. inserts generated rows, with NULLs and near-miss values (the generator
   `agreement` already has);
3. for each subject and each command, compares two sets:
   - **app**: as the test's superuser, which bypasses RLS,
     `SELECT id … WHERE <condition(subject)>` (not `row_security = off`:
     that raises an error on a forced table, it does not bypass);
   - **database**: `transact_as(identity(subject))`, `SELECT id …` with no
     `WHERE`;
   - for `create`: each candidate row, `INSERT` in a savepoint, against
     `create.eval` on the candidate;
   - for `update`/`delete`: each row, the statement in a savepoint, rows
     affected against the app's admission.
4. asserts each pair is equal, and prints the subject, the rows and both
   SQL texts when not.

It also unsets one setting (layer 2) and runs one query with no
`transact_as` (expects `permission denied`).

Reason: real Postgres catches what review misses: `NOT`/NULL folding, the
`SELECT`-on-`UPDATE` rule, a `WITH CHECK` that refuses a legal write. The
obligation moves to CI, as PLAN.md §9 asks. Each test owns its schema.

`brocanth-rls` runs it over fixed rules too, so this repo proves the
translator.

Built in phase 5 (18 November 2026). The API:

```rust
// brocanth-rls, feature `agreement`
pub fn check<P: DbPolicy>(url: &str, subjects: &[P::Subject], cases: u32) -> Result<(), String>;
pub fn run<P: DbPolicy>(url: Option<String>, subjects: &[P::Subject], cases: u32); // `db { … }` calls it
pub fn test_database_url() -> Option<String>; // BROCANTH_TEST_DATABASE_URL
```

```rust
db {
    subjects: [Visitor::anonymous(), Visitor::user(1)],
    cases: 64,                                   // optional, default 64
    url: crate::plugins::rls::test_url(),        // optional: Option<String>
}
```

`url` is the hook for an application: an expression of type
`Option<String>`, `None` skips. Its default is `test_database_url()`. The
scaffold (phase 6) points it at a function that reads `[db] url` through
the settings layers. Reason: the macro cannot name the app's settings, and
the compiler checks a path the scaffold writes.

Changes to the sketch above, and what the harness found:

- **The app side reads `rules()` resolved, not the `Policy` methods.**
  With `#[db]` they are the same code (decision 2), and `update_check`
  has no method. `create` uses `eval` on the candidate; a rule with
  `exists` has no `eval`, so the harness asks the condition with the
  candidate in the table.
- **`UPDATE` has three outcomes**, not "rows affected": done, no row
  (`USING`), or refused (`WITH CHECK`). The harness compares the outcome
  with `update` on the stored row and `update_check` on the new row. The
  new row is the next generated row's values.
- **Postgres checks the new row of an `UPDATE` against the `SELECT`
  policy too**, when the statement reads a column (`WHERE id = $1` does).
  So `update_check` must stay inside `list OR view`, as `update` must. The
  harness fails with a `help:` line on either.
- **Bug found: an empty text attribute.** `identity()` sets an absent
  value to `''`, and a transaction cannot unset a setting. So a subject
  with `role: String::new()` was "absent" in the database and `""` in the
  application: `who.role == "lead" => …` took the next arm in the app and
  denied the whole rule in the database. Fixed in phase 1's
  `Attributes::set`: an empty text is `None`, in both places. Reason:
  `''` is the only absence marker a setting has; an encoding for text
  settings would change every hand-written `Setting` reader.
- **A link table needs `SELECT` for the role.** The `exists` subquery of
  a policy runs as the request's role. Phase 5 granted it in the harness
  and left it to the developer; decision 15 moved it into `from_rules`.
- **The index is kept for a comparison with a setting** (`EXPLAIN` in
  `brocanth-rls/tests/bridge_postgres.rs`, `bigint` and `uuid`): the
  setting read is `STABLE`, so it is an index condition. Phase 5 found
  that a guard chain lost it: `s(admin) IS NOT NULL AND (admin OR (NOT
  admin AND col = s(x)))` planned as a sequential scan, because the
  `admin` arm reads no column. Decision 14 fixes it with a key gate.
- **Each case** creates its own schema; each run creates two roles,
  `brocanth_agree_<pid>_<n>_member` and `…_login` (`NOINHERIT`), and drops
  them. The URL's user must be a superuser: it creates roles and reads
  around the policies.
- **An `#[app_only]` command is left out**, with a note on stderr: it has
  no database rule to compare. So is a write command whose unguarded mask
  permits no column (phase 7): the application writes nothing.
- **Write masks (phase 7).** An `UPDATE` sets only the columns of an
  unguarded `writable` mask, as the application does. An `INSERT` writes
  every column, so the harness grants `INSERT` on the whole table: it
  compares rows, and `tests/masks.rs` tests the column grants.

### Decision 16: the agreement test fails on a forgotten `update_check`.

Without `update_check`, the database checks the new row of an `UPDATE`
with `update`. The application does not read the new row: the `writable`
mask does that job (decision 8). So a write the mask permits can move the
row out of `update`. The application writes it, and the database refuses
it. The comparison of the rules as data read `update` on both sides, so it
agreed.

The check: when the policy writes no `update_check`, for each subject, each
row that `update` admits, each column that `update` reads and the
subject's `writable` mask permits, and each sampled value of that column,
the harness runs an `UPDATE` of that column alone. When the new row leaves
`update` and the database refuses it, the test fails with the subject, the
row, the value, the policy SQL and:

```
help: write `update_check` with every row an update may leave (for a status change: the old and the new status), or remove `status` from `writable`
```

The mask is the subject's own: `DbPolicy::writable_of(&subject)`, which
`policy!` implements as `Policy::writable`, so a guarded mask (the
scaffold's example) counts. A hand-written `DbPolicy` returns `None` there,
and the harness then reads an unguarded `MaskRule::Fixed`; a guarded mask
is unknown and not checked. With no mask written, the application writes
no column, so there is nothing to check. Deleting `update_check` from the
example's shape (`tests/agreement.rs`, `PublishNoCheck`) fails the test.

Reasons:

- It reports only a real disagreement: one write that the application
  accepts and the database refuses, shown as data.
- With `update_check` written, the author chose the database's floor. It
  may be stricter than the mask on purpose, for example a status value the
  application does not check. So the check does not run. To keep a refusal
  on purpose, write `update_check` (it may repeat `update`), and remove the
  column from `writable` so the application refuses the write too.
- Rejected: a `rls make` warning when `update` reads a column the
  `writable` mask permits and `update_check` is absent. `rls make` cannot
  read a guarded mask: it is Rust, per subject, and the scaffold's example
  mask is guarded, so the warning would miss the case it is for. It also
  cannot tell an arm where no write leaves `update` (the admin's `always`)
  from one where a write does, so it would warn on correct policies.
- Rejected: a compile error in `policy!`, for the same reasons. It would
  refuse a policy that is correct for every subject.

Cost: the check runs only without `update_check`, over the columns
`update` reads. A value that keeps the row in `update` costs no statement.

## 5. Generated code, migrations, and what stays manual

### Decision 11: the `rls` module reads the policies by code reference.

module-config.md §9 decision 1: a module reads another module's choice in
generated code, at the place it uses it. With `project.has("policy")`, the
`rls` scaffold writes into `src/plugins/rls/mod.rs`:

```rust
pub const APP_USER: Role = Role::new("app_user").member("app_login");

pub fn schema() -> Schema {
    Schema::new()
        .role(APP_USER)
        .policies(Policy::from_rules::<crate::plugins::policy::PostPolicy>(&APP_USER))
}
```

The policy scaffold puts `#[db]` and `db { … }` on its example; `rls`
gains `policy` under "Adapts to". Reason: the policy file is the one source
and the compiler checks the path; a copy goes stale.

A new policy needs one `.policies(…)` line: no registry (ADR 1). A
forgotten line on a declared, forced table fails closed (no rows, tests
fail). If the table is forgotten too, it has no RLS, as today.

Built in phase 6 (18 November 2026), with these changes to the sketch:

- The role is `app_user()`, a function (phase 4), and it has no
  `.member(…)`: the login is the project's choice. The file says to add
  it.
- The example key is a `bigserial`, so the line ends with
  `.sequence("posts_id_seq")`.
- The example policy: `update` admits the author's draft, `update_check`
  admits the author's row as a draft or published, and `writable` holds
  `body` and `status`. So an author may publish a draft, and the database
  refuses a change of owner.
- With `rls`, the example has a `db { … }` clause over three subjects
  (anonymous, an author, an admin). Its `url` is
  `crate::plugins::rls::agreement_url()`: `[db] url` through every
  settings layer, as `rls check` reads it, or `None` (`!! SKIPPED`) when
  the URL is empty or nothing answers at its host and port within a
  second. Reason: a config project always has a `[db] url`, so "no URL"
  alone would fail `cargo test` on a machine with no Postgres. The URL's
  user must be a superuser; the generated docs say so.
- `brocanth-rls` takes the `agreement` feature in `[dev-dependencies]`
  when the project has `policy` (`Dep::for_dev`). Reason: only the test
  calls it, so `cargo build` compiles no proptest. Until the scaffold had
  dev-dependencies it was a normal dependency feature, and the server
  compiled proptest and linked none of it.
- The agreement test compared the rules as data on both sides, so it did
  not see a missing `update_check` when the app's `writable` mask permits
  the change: without `update_check`, both sides read `update` and agree.
  Decision 16 closes it. The project's own unit test
  (`an_author_publishes_a_draft_…`) still covers the example's case
  without a database.
- The project's own test checks that `schema().snapshot()` succeeds and
  that `check_identity` accepts the anonymous identity: a rule the
  database cannot check, or a key no policy reads, fails `cargo test`.
- The scaffold writes no migration. The first `rls make` alters `posts`,
  which the developer creates. Until that first `rls make` there is no
  `migrations/rls.json`, and generated CI skips `rls diff` with a
  `!! SKIPPED` line and a `help:`, so a new project starts green. Reason: a snapshot with no migration would say the
  database has policies it does not have, and a migration that creates
  `posts` is the application's schema, not the `rls` module's.

### Migration flow

Edit the rule, `rls make <name>` (snapshot diff, rls.md §3, unchanged),
`migrate up`.

Decision 12: `rls diff` exits non-zero when `schema()` differs
from `migrations/rls.json`; generated CI runs it. Reason: a rule edit
without a migration moves the app's `WHERE` and leaves the database on the
old rule. It needs no database.

### What stays manual

- Roles beyond `app_user`, `BYPASSRLS` jobs, and the login's membership.
- Functions (`Function::new`) a rule needs that the algebra does not have.
- `#[app_only]` rules: their database floor is a hand-written `SqlExpr`.
- Guarded write masks (decision 9), schema `USAGE`.
- Hand-written `impl Policy` (decision 1): write `crud(…)` with `SqlExpr`.
- Background jobs and commands: they choose an identity, or run as a role
  that bypasses on purpose. The bridge does not guess.

## 6. Plan

Sizes are focused hours for one person, with tests.

| Phase | Work | Hours |
|---|---|---|
| 0 | `feat/rls` lands the target API. Done (54db8067). | — |
| 1 | `brocanth-policy`: `Attr`, `Attributes`, `#[derive(Subject)]` (in `brocanth-policy-macros`), the `Rules<E>` tree (guards + `Operand::{Value, Attr}`) and `resolve`; `Uuid` and `Timestamptz` kinds. No change to `Pred` or `condition()`. Done. | 9 |
| 2 | `policy!` `#[db]`: emit `rules()` and `impl Policy` via `resolve`; `update_check`; `exists` syntax; the refusals of decision 3 with `help:` lines; strip `#[db]`/`#[app_only]` from the emitted struct; trybuild tests. Done. | 14 |
| 3 | `brocanth-rls` (depends on `brocanth-policy` under `runtime` only): `SqlExpr::from_rule`, guard chains, `Policy::from_rules`, grants with sequence `USAGE`, `.manual(…)`, the `#[app_only]` refusal at `rls make`, the owned-`no_force` report in `rls check`. Goldens. Done. | 8 |
| 4 | `brocanth_rls::identity`, the request provider, the sqlx and SeaORM call sites in the scaffold; `ResourceData` and the admin data source run as the identity (decision 13). Done. | 10 |
| 5 | Agreement on Postgres: the harness in `brocanth-rls`, the `db { … }` clause, `EXPLAIN` check on the example (index kept). | 9 |
| 6 | Scaffold: `rls` reads `policy`, `rls diff` in CI, goldens re-blessed, docs/rls.md §5 and docs/policy.md updated, ADR in PLAN.md §13. Done. | 6 |
| 7 | Optional: column grants for unguarded masks. Done. | 4 |

Total: about 56 hours (60 with phase 7). Phase 5 lands before any
project turns `#[db]` on.

## 7. Owner answers (17 November 2026)

1. **Macro-only:** yes. A hand-written `impl Policy` writes `crud(…)` with
   `SqlExpr`.
2. **One role** with guards on settings. A role per kind is as trusted by
   the app as a setting, and costs a grant and a migration per kind.
3. **`UPDATE … WITH CHECK`:** `update_check`, which defaults to `update`;
   it ships in phase 2 (decision 8).
4. **`SELECT` = `list OR view`.** The agreement test fails when `update`
   or `delete` reach a row outside it.
5. **Masks** app-only in v1; column grants for unguarded masks in phase 7.
6. **Kinds:** add `Uuid` and `Timestamptz` (decision 5).
7. **Crates:** `brocanth-rls` depends on `brocanth-policy` under `runtime`
   only; `brocanth-db` keeps `SettingKey`. No cycle: `brocanth-policy` has
   no brocanth dependency. The config `Setting` of module-capabilities.md
   is a different type with the same name.
8. **`else allow` on an attribute:** refused with `#[db]` (decision 3).
9. **App agreement test:** on by default, skipped with `!! SKIPPED` when
   no database URL is set (decision 10).

Added after review: decision 13 (identity on resource and admin paths),
the owner-login precondition of layer 1, `exists` syntax, `rls diff`, and
the sequence grant.
