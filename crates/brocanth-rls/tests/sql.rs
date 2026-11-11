//! The SQL this crate writes, against golden files, and the checks a schema
//! must pass. No database.

mod common;

use brocanth_rls::migration::{self, Format};
use brocanth_rls::{
    diff, Function, Policy, Privilege, Role, Schema, Snapshot, SqlExpr, SqlType, Table,
};
use common::{changed, example, golden, member, notes, posts, Names};

fn names() -> Names {
    Names::new("app")
}

#[test]
fn the_whole_schema_is_created_in_dependency_order() {
    let sql = example(&names()).create_sql().unwrap();
    golden("create.sql", &sql);
    // Each node after what it needs.
    let at = |needle: &str| {
        sql.find(needle)
            .unwrap_or_else(|| panic!("no `{needle}` in\n{sql}"))
    };
    assert!(at("CREATE ROLE \"app_member\"") < at("GRANT SELECT"));
    assert!(at("CREATE OR REPLACE FUNCTION") < at("CREATE POLICY \"posts_tenant\""));
    assert!(at("ENABLE ROW LEVEL SECURITY") < at("CREATE POLICY"));
}

#[test]
fn equal_snapshots_make_no_step() {
    let s = example(&names()).snapshot().unwrap();
    assert!(diff(&s, &s).unwrap().is_empty());
}

#[test]
fn the_snapshot_survives_json() {
    let s = example(&names()).snapshot().unwrap();
    let text = s.to_json();
    golden("rls.json", &text);
    assert_eq!(Snapshot::from_json(&text).unwrap(), s);
}

#[test]
fn a_change_is_the_steps_it_needs_in_both_directions() {
    let n = names();
    let old = example(&n).snapshot().unwrap();
    let new = changed(&n).snapshot().unwrap();
    let up = diff(&old, &new).unwrap();
    let down = diff(&new, &old).unwrap();
    golden("change.sql", &migration::sql_file(&up, &down).unwrap());
    // A command change cannot be altered: drop, then create.
    let steps = up.describe();
    let drop = steps
        .iter()
        .position(|s| s == "drop policy posts_tenant on app.posts")
        .expect("dropped");
    let create = steps
        .iter()
        .position(|s| s == "create policy posts_tenant on app.posts")
        .expect("created");
    assert!(drop < create);
    assert!(steps.contains(&"alter policy notes_app_member_select on app.notes".to_string()));
    assert!(steps.contains(&"drop policy notes_app_member_insert on app.notes".to_string()));
    assert!(steps.contains(&"create role app_staff".to_string()));
    // The role goes after the policy that names it, the other way down.
    let down_steps = down.describe();
    let policy = down_steps
        .iter()
        .position(|s| s == "drop policy notes_staff on app.notes")
        .unwrap();
    let role = down_steps
        .iter()
        .position(|s| s == "drop role app_staff")
        .unwrap();
    assert!(policy < role, "{down_steps:#?}");
}

#[test]
fn a_function_with_a_new_signature_takes_its_policies_with_it() {
    let n = names();
    let old = example(&n).snapshot().unwrap();
    let posts = posts(&n);
    let member = member(&n);
    let f = Function::new("current_tenant", SqlType::Text)
        .in_schema(&n.schema)
        .body(SqlExpr::val("x"));
    let new = Schema::new()
        .role(member.clone())
        .role(Role::existing(&n.login))
        .function(f.clone())
        .table(posts.clone())
        .table(notes(&n))
        .policy(
            Policy::new("posts_tenant", &posts)
                .to(&member)
                .using(SqlExpr::col("title").eq(f.call([]))),
        )
        .snapshot()
        .unwrap();
    let steps = diff(&old, &new).unwrap().describe();
    let at = |s: &str| {
        steps
            .iter()
            .position(|x| x == s)
            .unwrap_or_else(|| panic!("{s}: {steps:#?}"))
    };
    assert!(at("drop policy posts_tenant on app.posts") < at("drop function app.current_tenant"));
    assert!(
        at("create function app.current_tenant") < at("create policy posts_tenant on app.posts")
    );
}

#[test]
fn a_seaorm_migration_runs_the_same_sql() {
    let n = names();
    let old = Snapshot::default();
    let new = example(&n).snapshot().unwrap();
    let text = migration::seaorm_file(
        "m20261111_000000_rls",
        &diff(&old, &new).unwrap(),
        &diff(&new, &old).unwrap(),
    );
    golden("m20261111_000000_rls.rs", &text);
}

#[test]
fn make_writes_a_file_and_the_snapshot_then_nothing() {
    let dir = common::temp_dir("make");
    let schema = example(&names());
    let made = migration::make(&schema, &dir, "add_rls", Format::Sql)
        .unwrap()
        .expect("a first migration");
    let text = std::fs::read_to_string(&made.path).unwrap();
    let parsed = brocanth_db::migrate::Migration::parse("x", &text);
    assert!(parsed.up.contains("CREATE POLICY"));
    assert!(parsed.down.unwrap().contains("DROP POLICY"));
    assert!(dir.join("rls.json").exists());
    assert_eq!(
        migration::make(&schema, &dir, "again", Format::Sql).unwrap(),
        None
    );
    let bad = migration::make(&Schema::new(), &dir, "Not-Snake", Format::Sql).unwrap_err();
    assert!(bad.to_string().contains("help:"), "{bad}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_schema_refuses_what_it_cannot_write_with_help() {
    let posts = Table::new("posts");
    let ghost = Role::new("ghost");
    let cases: Vec<(&str, Schema)> = vec![
        (
            "undeclared table",
            Schema::new().policy(Policy::new("p", &posts)),
        ),
        (
            "undeclared role",
            Schema::new()
                .table(posts.clone())
                .policy(Policy::new("p", &posts).to(&ghost)),
        ),
        (
            "undeclared grantee",
            Schema::new().table(posts.clone().grant(&ghost, Privilege::CRUD)),
        ),
        (
            "two policies, one name",
            Schema::new()
                .table(posts.clone())
                .policy(Policy::new("p", &posts))
                .policy(Policy::new("p", &posts)),
        ),
        (
            "a keyword as a role",
            Schema::new().role(Role::new("public")),
        ),
        (
            "a long name",
            Schema::new().table(Table::new("t".repeat(64))),
        ),
        (
            "a setting read as two types",
            Schema::new().table(posts.clone()).policy(
                Policy::new("p", &posts).using(
                    common::tenant().get().eq(brocanth_rls::Setting::new(
                        "tenant_id",
                        SqlType::Text,
                    )
                    .get()),
                ),
            ),
        ),
        (
            "a bad setting key",
            Schema::new().table(posts.clone()).policy(
                Policy::new("p", &posts).using(
                    brocanth_rls::Setting::new("Tenant", SqlType::BigInt)
                        .get()
                        .is_null(),
                ),
            ),
        ),
        (
            "a type that ends the statement",
            Schema::new().table(posts.clone()).policy(
                Policy::new("p", &posts).using(
                    SqlExpr::col("x")
                        .cast(SqlType::Custom("int); DROP TABLE x; --".into()))
                        .is_null(),
                ),
            ),
        ),
        (
            "an undeclared function",
            Schema::new().table(posts.clone()).policy(
                Policy::new("p", &posts).using(
                    Function::new("f", SqlType::Boolean)
                        .body(SqlExpr::always())
                        .call([]),
                ),
            ),
        ),
        ("a function cycle", {
            let a = Function::new("a", SqlType::Boolean);
            let b = Function::new("b", SqlType::Boolean).body(a.call([]));
            let a = a.body(b.call([]));
            Schema::new().function(a).function(b)
        }),
    ];
    for (what, schema) in cases {
        let e = schema.snapshot().expect_err(what).to_string();
        assert!(e.contains("\nhelp: "), "{what}: {e}");
        assert_eq!(e.matches("help:").count(), 1, "{what}: {e}");
    }
}

#[test]
fn a_principal_is_checked_against_the_settings_the_policies_read() {
    let schema = example(&names());
    let ok = brocanth_rls::Principal::new()
        .role("app_member")
        .set("tenant_id", 1)
        .set("user_id", 2);
    schema.check_principal(&ok).unwrap();
    let typo = brocanth_rls::Principal::new().set("tenantid", 1);
    let e = schema.check_principal(&typo).unwrap_err().to_string();
    assert!(e.contains("tenant_id") && e.contains("help:"), "{e}");
    let role = brocanth_rls::Principal::new().role("nobody");
    assert!(schema.check_principal(&role).is_err());
}

#[test]
fn a_value_with_a_line_break_cannot_split_the_file() {
    let posts = Table::new("posts");
    let schema = Schema::new().table(posts.clone()).policy(
        Policy::new("p", &posts).using(SqlExpr::col("title").eq(SqlExpr::val("a\n-- down\nb"))),
    );
    let up = diff(&Snapshot::default(), &schema.snapshot().unwrap()).unwrap();
    // sea-query writes the line break as an escape, so the file stays one
    // statement per line; the check stays as the floor.
    match migration::sql_file(&up, &up) {
        Ok(text) => assert_eq!(text.matches("\n-- down\n").count(), 1, "{text}"),
        Err(e) => assert!(e.to_string().contains("help:")),
    }
}
