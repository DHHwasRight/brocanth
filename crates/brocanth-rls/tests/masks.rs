//! Write masks as column grants: docs/policy-rls-bridge.md, decision 9.
//!
//! A `writable` mask with no guard grants `UPDATE` on its columns instead
//! of the table; `writable_on_create` does the same for `INSERT`. A mask
//! with a guard keeps the table grant and warns. No database: the text,
//! the snapshot and the diff. `bridge_postgres.rs` runs them.
#![allow(missing_docs)]

mod common;

use brocanth_policy::{FieldMask, MaskRule, Rules};
use brocanth_rls::snapshot::TableSpec;
use brocanth_rls::{diff, Policy, Privilege, Role, RulePolicies, Schema, Snapshot, Table};
use common::golden;
use common::policy::{db_policy, post_rules, PostColumn};

use PostColumn::*;

fn app_user() -> Role {
    Role::new("app_user").member("app_login")
}

fn schema(policies: RulePolicies) -> Schema {
    Schema::new()
        .role(app_user())
        .role(Role::existing("app_login"))
        .policies(policies)
}

fn with_masks(
    writable: Option<MaskRule<PostColumn>>,
    on_create: Option<MaskRule<PostColumn>>,
) -> Rules<common::policy::Post> {
    let mut rules = post_rules();
    rules.writable = writable;
    rules.writable_on_create = on_create;
    rules
}

/// `writable: only [title, status]`, `writable_on_create: only [title,
/// author_id, tenant_id]`: the same for every subject.
pub struct Fixed;
db_policy!(
    Fixed,
    with_masks(
        Some(MaskRule::Fixed(FieldMask::only([Title, Status]))),
        Some(MaskRule::Fixed(FieldMask::only([
            Title, AuthorId, TenantId
        ]))),
    )
);

/// `writable { who.admin => all, only [title] }`.
pub struct Guarded;
db_policy!(Guarded, with_masks(Some(MaskRule::PerSubject), None));

fn posts(snapshot: &Snapshot) -> &TableSpec {
    &snapshot.tables["posts"]
}

#[test]
fn an_unguarded_mask_grants_its_columns_instead_of_the_table() {
    let s = schema(Policy::from_rules::<Fixed>(&app_user()).sequence("posts_id_seq"));
    assert!(s.warnings().is_empty(), "{:?}", s.warnings());
    let sql = s.create_sql().unwrap();
    golden("from_rules_masks.sql", &sql);
    assert!(
        sql.contains(r#"GRANT SELECT ON "posts" TO "app_user""#),
        "{sql}"
    );
    assert!(
        sql.contains(
            r#"GRANT INSERT ("author_id", "tenant_id", "title") ON "posts" TO "app_user""#
        ),
        "{sql}"
    );
    assert!(
        sql.contains(r#"GRANT UPDATE ("status", "title") ON "posts" TO "app_user""#),
        "{sql}"
    );
    // Never both: no table-wide INSERT or UPDATE.
    assert!(!sql.contains(r#"INSERT ON "posts""#), "{sql}");
    assert!(!sql.contains(r#"UPDATE ON "posts""#), "{sql}");
    // The policies do not change, and a column INSERT still needs the
    // sequence.
    assert!(sql.contains(r#""posts_app_user_insert""#), "{sql}");
    assert!(sql.contains(r#""posts_app_user_update""#), "{sql}");
    assert!(
        sql.contains(r#"GRANT USAGE ON SEQUENCE "posts_id_seq" TO "app_user""#),
        "{sql}"
    );
}

#[test]
fn writable_on_create_defaults_to_writable_and_except_lists_the_rest() {
    pub struct Except;
    db_policy!(
        Except,
        with_masks(
            Some(MaskRule::Fixed(FieldMask::except([Id, TenantId]))),
            None
        )
    );
    let snapshot = schema(Policy::from_rules::<Except>(&app_user()))
        .snapshot()
        .unwrap();
    let cols = &posts(&snapshot).column_grants["app_user"];
    let want = ["author_id", "status", "title"].map(String::from).into();
    assert_eq!(cols[&Privilege::Insert], want);
    assert_eq!(cols[&Privilege::Update], want);
    assert_eq!(
        posts(&snapshot).grants["app_user"],
        [Privilege::Select].into()
    );
}

#[test]
fn an_all_mask_or_no_mask_keeps_the_table_grant() {
    pub struct All;
    db_policy!(All, with_masks(Some(MaskRule::Fixed(FieldMask::All)), None));
    for s in [
        schema(Policy::from_rules::<All>(&app_user())),
        schema(Policy::from_rules::<common::policy::PostPolicy>(&app_user())),
    ] {
        assert!(s.warnings().is_empty());
        let snapshot = s.snapshot().unwrap();
        assert!(posts(&snapshot).column_grants.is_empty());
        assert_eq!(
            posts(&snapshot).grants["app_user"],
            [Privilege::Select, Privilege::Insert, Privilege::Update].into()
        );
    }
}

#[test]
fn a_mask_that_permits_no_column_writes_no_policy_and_no_grant() {
    pub struct NoWrite;
    db_policy!(
        NoWrite,
        with_masks(Some(MaskRule::Fixed(FieldMask::None)), None)
    );
    let snapshot = schema(Policy::from_rules::<NoWrite>(&app_user()))
        .snapshot()
        .unwrap();
    let t = posts(&snapshot);
    assert_eq!(t.grants["app_user"], [Privilege::Select].into());
    assert!(t.column_grants.is_empty());
    assert!(!t.policies.contains_key("posts_app_user_update"));
    assert!(!t.policies.contains_key("posts_app_user_insert"));
}

#[test]
fn a_guarded_mask_keeps_the_table_grant_and_warns_once_per_table() {
    let s = schema(Policy::from_rules::<Guarded>(&app_user()));
    assert_eq!(
        s.warnings(),
        ["posts: app_user may INSERT and UPDATE any column; its write mask has a guard, so only the application checks it (docs/policy-rls-bridge.md, decision 9)"]
    );
    let snapshot = s.snapshot().unwrap();
    assert!(posts(&snapshot).column_grants.is_empty());
    assert_eq!(
        posts(&snapshot).grants["app_user"],
        [Privilege::Select, Privilege::Insert, Privilege::Update].into()
    );

    // A guard on a command the rules deny leaves no gap: no warning.
    pub struct GuardedReadOnly;
    db_policy!(GuardedReadOnly, {
        let mut rules = Rules::deny_all();
        rules.list = post_rules().list;
        rules.writable = Some(MaskRule::PerSubject);
        rules
    });
    assert!(schema(Policy::from_rules::<GuardedReadOnly>(&app_user()))
        .warnings()
        .is_empty());
}

#[test]
fn a_snapshot_without_column_grants_keeps_its_json_and_one_with_them_reads_back() {
    let plain = schema(Policy::from_rules::<common::policy::PostPolicy>(&app_user()))
        .snapshot()
        .unwrap();
    assert!(!plain.to_json().contains("column_grants"));
    let masked = schema(Policy::from_rules::<Fixed>(&app_user()))
        .snapshot()
        .unwrap();
    let json = masked.to_json();
    assert!(json.contains(r#""column_grants": {"#), "{json}");
    assert!(json.contains(r#""UPDATE": ["#), "{json}");
    assert_eq!(Snapshot::from_json(&json).unwrap(), masked);
}

#[test]
fn the_diff_moves_between_table_and_column_grants_in_a_safe_order() {
    let table = schema(Policy::from_rules::<common::policy::PostPolicy>(&app_user()))
        .snapshot()
        .unwrap();
    let columns = schema(Policy::from_rules::<Fixed>(&app_user()))
        .snapshot()
        .unwrap();

    // Table to columns: the table REVOKE runs first, because it also
    // revokes the column privileges.
    let up = diff(&table, &columns).unwrap().sql();
    golden("masks_up.sql", &up);
    let revoke = up
        .find(r#"REVOKE INSERT, UPDATE ON "posts" FROM "app_user""#)
        .expect(&up);
    let grant = up
        .find(r#"GRANT UPDATE ("status", "title") ON "posts" TO "app_user""#)
        .expect(&up);
    assert!(revoke < grant, "{up}");

    // And back.
    let back = diff(&columns, &table).unwrap().sql();
    assert!(
        back.contains(r#"REVOKE UPDATE ("status", "title") ON "posts" FROM "app_user""#),
        "{back}"
    );
    assert!(
        back.contains(r#"GRANT INSERT, UPDATE ON "posts" TO "app_user""#),
        "{back}"
    );

    // A column more, a column less: one REVOKE and one GRANT of those.
    pub struct Moved;
    db_policy!(
        Moved,
        with_masks(
            Some(MaskRule::Fixed(FieldMask::only([Title, AuthorId]))),
            Some(MaskRule::Fixed(FieldMask::only([
                Title, AuthorId, TenantId
            ]))),
        )
    );
    let moved = schema(Policy::from_rules::<Moved>(&app_user()))
        .snapshot()
        .unwrap();
    let plan = diff(&columns, &moved).unwrap();
    assert_eq!(
        plan.describe(),
        ["alter grant UPDATE on columns of posts to app_user"]
    );
    assert_eq!(
        plan.sql(),
        "-- alter grant UPDATE on columns of posts to app_user\n\
         REVOKE UPDATE (\"status\") ON \"posts\" FROM \"app_user\";\n\
         GRANT UPDATE (\"author_id\") ON \"posts\" TO \"app_user\";\n"
    );
    assert!(diff(&moved, &moved).unwrap().is_empty());
}

#[test]
fn a_table_and_column_grant_of_one_privilege_and_column_delete_are_refused() {
    let role = app_user();
    let both = Schema::new().role(role.clone()).table(
        Table::named("posts")
            .grant(&role, [Privilege::Update])
            .grant_columns(&role, Privilege::Update, ["title"]),
    );
    let e = both.snapshot().expect_err("refused").to_string();
    assert!(e.contains("on the table and on columns"), "{e}");
    assert!(e.contains("help: keep one"), "{e}");

    let delete = Schema::new()
        .role(role.clone())
        .table(Table::named("posts").grant_columns(&role, Privilege::Delete, ["title"]));
    let e = delete.snapshot().expect_err("refused").to_string();
    assert!(e.contains("DELETE removes a whole row"), "{e}");

    let none = Schema::new()
        .role(role.clone())
        .table(Table::named("posts").grant_columns(&role, Privilege::Update, Vec::<String>::new()));
    let e = none.snapshot().expect_err("refused").to_string();
    assert!(e.contains("help: name at least one column"), "{e}");

    let stranger = Schema::new().table(Table::named("posts").grant_columns(
        &Role::new("nobody"),
        Privilege::Update,
        ["title"],
    ));
    let e = stranger.snapshot().expect_err("refused").to_string();
    assert!(e.contains("not a role of the schema"), "{e}");
}
