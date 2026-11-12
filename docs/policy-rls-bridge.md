# One rule, two enforcers: the policy-RLS bridge

Written 12 November 2026. Status: **design, not built**. Postgres only.

Owner decision: one rule drives the application's `WHERE` (`brocanth-policy`
`condition()` for lists, `Verified` for writes) and the database policy
(`brocanth-rls`). The author writes the rule once.

This doc uses the **target API of `feat/rls`**, which a concurrent change
writes now: `DbIdentity` (was `Principal`), typed `&Setting` setters,
`is()` refused on a setting, typed SeaORM columns, `Orm::transact_as`. It
replaces the sketch in `docs/rls.md` §5.

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
| `col == who.x else allow` | `(s(x) IS NULL OR "col" = s(x))` | allowed; the role still gates (§4) |
| `col == who.x else null` | `"col" = s(x)` | three-valued, as the app |
| `col == who.x` (non-`Option` field) | `(s(x) IS NOT NULL AND "col" = s(x))` | a NULL setting fails closed |
| `col is lit`, `col is null` | `IS NOT DISTINCT FROM 'lit'`, `IS NULL` | literal only |
| `col in [lits]`, `not in` | `IN (…)`, `NOT IN (…)` | literal only |
| `&&`, `\|\|`, `!` | `AND`, `OR`, `NOT` | |
| `Pred::Exists` (membership) | `EXISTS (SELECT 1 FROM "t" WHERE …)` | filter values may be `who.x` |
| guard `who.f`, `!who.f`, `who.f == lit`, `&&`/`\|\|` of them | boolean over `s(f)` | see below |

A guard chain `g1 => r1, g2 => r2, f` becomes:

```sql
s(g1) IS NOT NULL AND s(g2) IS NOT NULL AND (
     (g1 AND r1)
  OR (NOT g1 AND g2 AND r2)
  OR (NOT g1 AND NOT g2 AND f))
```

A term with `never` drops out. A NULL guard setting makes the rule FALSE.
Reason: a Rust `bool` has no third value; without the prefix, a missing
`brocanth.banned` reads as "not banned". The form is boolean, not `CASE`,
so the planner sees plain `AND`/`OR` (phase 5 checks the index is kept).

### Decision 3: these shapes are refused, at compile time with `#[db]`, or at `rls make`.

| Shape | Refused where | `help:` line |
|---|---|---|
| `{ rust }` block in a rule or a guard | macro | `help: write the rule in the policy! syntax, or mark it #[app_only("reason")]` |
| a guard that is not `who.<field>`, `!`, `== lit`, `&&`, `\|\|` | macro | `help: add the value to the subject as a #[attr] field and test that` |
| a value that uses `who` and is not exactly `who.<field>` | macro | `help: compute the value when you build the subject, and store it in a field` |
| `col is who.x`, `col is not who.x` | macro | `help: use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting` |
| `col in who.xs` (a list attribute) | macro | `help: a setting holds one value; use a membership table and Pred::Exists` |
| `Pred::Opaque` | macro (it is a `{ }` block) | as the first row |
| a `#[db]` policy whose subject field has no `#[attr]` | compiler (the accessor does not exist) | the derive's error |
| an `#[app_only]` rule on a command | `rls make` | `help: write a SqlExpr for <table> <command> in src/plugins/rls/mod.rs with .manual(...)` |

Reason for the `is` refusal: `NULL IS NOT DISTINCT FROM NULL` is TRUE, so
a request with no `user_id` matches every row with no author. `feat/rls`
refuses the same on `SqlExpr`.

### Decision 4: an app-only rule never becomes open or closed by default.

`#[app_only("reason")]` on one rule lets the app keep a Rust-only check.
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
`i64`/`i32` -> `bigint`/`integer`, `bool` -> `boolean`, `String` -> `text`.
`brocanth-rls` turns each `Attr` into a typed `Setting`.

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

## 3. Commands

`brocanth_rls::Policy::from_rules::<P>(&role)` writes up to four policies,
named `<table>_<role>_<command>`, like `crud`.

| `Policy` method | Database policy | Reason |
|---|---|---|
| `list` OR `view` | `FOR SELECT USING (list OR view)` | Postgres has one read command. The database floor is the union. The app narrows a list with `list`. |
| `create` | `FOR INSERT WITH CHECK (create)` | both check the proposed row |
| `update` | `FOR UPDATE USING (update) WITH CHECK (update)` | see decision 8 |
| `delete` | `FOR DELETE USING (delete)` | |
| a rule that is `never` for every guard | no policy, no grant | RLS denies what no policy allows. No grant gives a clear `permission denied`. |

`from_rules` also grants the commands that have a policy, and forces RLS.

### Decision 8: `UPDATE … WITH CHECK` uses the `update` rule.

The app's `update` checks the stored row only (policy.rs: "decides on the
row as stored"). The `writable` mask is what stops an author from moving a
post to somebody else. The database adds `WITH CHECK (update)` on the new
row too.

Reason: the extra strictness covers the hole the mask covers, so the
database refuses an owner change when a write path forgets the mask. Cost:
`update: status == "draft"` with `status` writable lets the app publish a
draft that the database refuses. §4's test finds it; the author then writes
`update_check: <pred>` (new, optional; `always` = stored row only).

Postgres also applies the `SELECT` policy to an `UPDATE`/`DELETE` that
reads a column, so a row you may update but not see stays unchanged. §4
reports it; the docs will say: keep `update` and `delete` inside `list`.

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

## 4. NULL, fail-closed, and the agreement test

### The layers that fail closed

1. **No identity, no grant.** Without `transact_as`, the query runs as the
   login role, which has no table privilege: `permission denied`.
2. **A missing non-`Option` attribute denies** (`s(x) IS NOT NULL AND …`).
3. **`else deny` is FALSE, not NULL.** The app folds `or_deny(None)` to
   `Never`, and `negate(Never)` is `Always`. `NOT (col = NULL)` is NULL, a
   deny: they would disagree. `(s IS NOT NULL AND col = s)` is FALSE, and
   `NOT FALSE` is TRUE, as in the app. Faithful beats "more closed".
4. **`else allow` is the author's word**, in both places. Layer 1 holds.

### Decision 10: an agreement test, on Postgres, per `#[db]` policy.

The `eval { … }` clause (docs/policy.md) proves `eval` agrees with
`condition()`. A new `db { … }` clause proves `condition()` agrees with RLS:

```rust
db {
    subjects: [Visitor::anonymous(), Visitor::user(1), Visitor::admin(), Visitor::banned(1)],
    cases: 64,
}
```

It is skipped without `BROCANTH_TEST_DATABASE_URL`. Per case it:

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

### Migration flow

Edit the rule, `rls make <name>` (snapshot diff, rls.md §3, unchanged),
`migrate up`.

Decision 12: `rls make --check` exits non-zero when `schema()` differs
from `migrations/rls.json`; generated CI runs it. Reason: a rule edit
without a migration moves the app's `WHERE` and leaves the database on the
old rule. It needs no database.

### What stays manual

- Roles beyond `app_user`, `BYPASSRLS` jobs, and the login's membership.
- Functions (`Function::new`) a rule needs that the algebra does not have.
- `#[app_only]` rules: their database floor is a hand-written `SqlExpr`.
- Guarded write masks (decision 9), sequence `USAGE`, schema `USAGE`.
- Hand-written `impl Policy` (decision 1): write `crud(…)` with `SqlExpr`.
- Background jobs and commands: they choose an identity, or run as a role
  that bypasses on purpose. The bridge does not guess.

## 6. Plan

Sizes are focused hours for one person, with tests.

| Phase | Work | Hours |
|---|---|---|
| 0 | `feat/rls` lands the target API (other change). This plan waits. | — |
| 1 | `brocanth-policy`: `Attr`, `Attributes`, `#[derive(Subject)]`, the `Rules<E>` tree (guards + `Operand::{Value, Attr}`) and `resolve`. No change to `Pred`, `Value` or `condition()`. | 6 |
| 2 | `policy!` `#[db]`: emit `rules()` and `impl Policy` via `resolve`; the refusals of decision 3 with `help:` lines; trybuild tests. | 10 |
| 3 | `brocanth-rls`: `SqlExpr::from_rule`, guard chains, `Policy::from_rules`, grants, `.manual(…)`, the `#[app_only]` refusal at `rls make`. Goldens. | 8 |
| 4 | `brocanth_rls::identity`, the request provider, the sqlx and SeaORM call sites in the scaffold. | 5 |
| 5 | Agreement on Postgres: the harness in `brocanth-rls`, the `db { … }` clause, `EXPLAIN` check on the example (index kept). | 9 |
| 6 | Scaffold: `rls` reads `policy`, `rls make --check` in CI, goldens re-blessed, docs/rls.md §5 and docs/policy.md updated, ADR in PLAN.md §13. | 6 |
| 7 | Optional: column grants for unguarded masks. | 4 |

Total: about 44 hours (48 with phase 7). Phase 5 lands before any
project turns `#[db]` on.

## 7. Open questions for the owner

1. **Macro-only.** Hand-written `impl Policy` gets no database policy. OK?
2. **One role** with guards on settings, or a role per kind of user (which
   makes column grants exact)?
3. **`UPDATE … WITH CHECK`**: the `update` rule (this doc) or `TRUE`?
4. **`SELECT` = `list OR view`**, or require `view == list` under `#[db]`?
5. **Masks** app-only in v1, column grants for unguarded masks later?
6. **Kinds**: `Value` has no `uuid`/timestamp. Add kinds, or `text` + cast?
7. **Crates**: `brocanth-rls` -> `brocanth-policy`; `brocanth-db` keeps only
   `SettingKey`. Confirm against the concurrent `Setting` work.
8. **`else allow` on an attribute**: permit (this doc) or refuse?
9. **App agreement test** needs Postgres in app CI: on by default or flag?
