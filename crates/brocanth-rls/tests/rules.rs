//! `SqlExpr::from_rule` and `Policy::from_rules`: the translation table of
//! docs/policy-rls-bridge.md §1, the commands of §3, and the refusals. No
//! database: the text, and a golden file for a whole policy.
#![allow(missing_docs)]

mod common;

use brocanth_policy::{
    Attr, DbPolicy, Exists, Guard, Ident, Missing, Op, Operand, Policy as _, Rule, Rules, Subject,
    Term, Value,
};
use brocanth_rls::{Command, Policy, Role, Schema, SqlExpr, Table};
use common::golden;
use common::policy::{
    attr, cmp, db_policy, lit, post_rules, PostColumn, PostDb, PostPolicy, Visitor,
};

use PostColumn::*;

const S_USER: &str = "CAST(NULLIF(current_setting('brocanth.user_id', TRUE), '') AS bigint)";
const S_TENANT: &str = "CAST(NULLIF(current_setting('brocanth.tenant_id', TRUE), '') AS bigint)";
const S_ADMIN: &str = "CAST(NULLIF(current_setting('brocanth.admin', TRUE), '') AS boolean)";
const S_BANNED: &str = "CAST(NULLIF(current_setting('brocanth.banned', TRUE), '') AS boolean)";
const S_ROLE: &str = "CAST(NULLIF(current_setting('brocanth.role', TRUE), '') AS text)";

fn sql(rule: Rule<PostColumn>) -> String {
    SqlExpr::from_rule(&rule, &Table::named("posts"), Visitor::ATTRS)
        .unwrap_or_else(|e| panic!("{e}"))
        .to_sql()
}

fn term(t: Term<PostColumn>) -> String {
    sql(Rule::new(t))
}

fn refused(rule: Rule<PostColumn>) -> String {
    SqlExpr::from_rule(&rule, &Table::named("posts"), Visitor::ATTRS)
        .expect_err("refused")
        .to_string()
}

#[test]
fn always_and_never_are_true_and_false() {
    assert_eq!(term(Term::Always), "TRUE");
    assert_eq!(term(Term::Never), "FALSE");
}

#[test]
fn a_literal_comparison_is_written_by_sea_query() {
    assert_eq!(
        term(cmp(Status, Op::Eq, lit("published"))),
        r#""status" = 'published'"#
    );
    assert_eq!(
        term(cmp(Title, Op::Ne, lit("it's"))),
        r#""title" <> E'it\'s'"#
    );
    for (op, text) in [(Op::Lt, "<"), (Op::Le, "<="), (Op::Gt, ">"), (Op::Ge, ">=")] {
        assert_eq!(
            term(cmp(AuthorId, op, lit(10i64))),
            format!(r#""author_id" {text} 10"#)
        );
    }
}

#[test]
fn else_deny_is_false_when_the_setting_is_absent() {
    assert_eq!(
        term(cmp(AuthorId, Op::Eq, attr("user_id", Missing::Deny))),
        format!(r#"{S_USER} IS NOT NULL AND "author_id" = {S_USER}"#)
    );
}

#[test]
fn else_null_is_three_valued() {
    assert_eq!(
        term(cmp(AuthorId, Op::Eq, attr("user_id", Missing::Null))),
        format!(r#""author_id" = {S_USER}"#)
    );
}

#[test]
fn a_field_that_is_not_an_option_fails_closed_whatever_its_mode() {
    assert_eq!(
        term(cmp(TenantId, Op::Eq, attr("tenant_id", Missing::Null))),
        format!(r#"{S_TENANT} IS NOT NULL AND "tenant_id" = {S_TENANT}"#)
    );
}

#[test]
fn is_on_a_literal_is_null_safe() {
    assert_eq!(
        term(cmp(Status, Op::Is, lit("x"))),
        r#""status" IS NOT DISTINCT FROM 'x'"#
    );
    assert_eq!(
        term(cmp(Status, Op::IsNot, lit("x"))),
        r#""status" IS DISTINCT FROM 'x'"#
    );
    assert_eq!(
        term(cmp(AuthorId, Op::Is, Operand::Value(Value::Null))),
        r#""author_id" IS NULL"#
    );
    assert_eq!(
        term(cmp(AuthorId, Op::IsNot, Operand::Value(Value::Null))),
        r#""author_id" IS NOT NULL"#
    );
    // `=` with NULL stays three-valued, as the app's `Pred::Cmp`.
    assert_eq!(
        term(cmp(AuthorId, Op::Eq, Operand::Value(Value::Null))),
        r#""author_id" = NULL"#
    );
}

#[test]
fn in_and_not_in_take_literals_and_an_empty_set_folds() {
    let set = |negated, values: Vec<Value>| Term::In {
        column: Status,
        values,
        negated,
    };
    assert_eq!(
        term(set(false, vec!["a".into(), "b".into()])),
        r#""status" IN ('a', 'b')"#
    );
    assert_eq!(
        term(set(true, vec!["a".into()])),
        r#""status" NOT IN ('a')"#
    );
    assert_eq!(term(set(false, vec![])), "FALSE");
    assert_eq!(term(set(true, vec![])), "TRUE");
}

#[test]
fn and_or_not_are_written_and_constants_fold() {
    let a = cmp(Status, Op::Eq, lit("a"));
    let b = cmp(Title, Op::Eq, lit("b"));
    assert_eq!(
        term(Term::All(vec![a.clone(), Term::Always, b.clone()])),
        r#""status" = 'a' AND "title" = 'b'"#
    );
    assert_eq!(
        term(Term::Any(vec![a.clone(), Term::Never, b.clone()])),
        r#""status" = 'a' OR "title" = 'b'"#
    );
    assert_eq!(
        term(Term::Not(Box::new(Term::Any(vec![a.clone(), b.clone()])))),
        r#"NOT ("status" = 'a' OR "title" = 'b')"#
    );
    assert_eq!(
        term(Term::All(vec![
            Term::Any(vec![a.clone(), b.clone()]),
            a.clone()
        ])),
        r#"("status" = 'a' OR "title" = 'b') AND "status" = 'a'"#
    );
    assert_eq!(term(Term::All(vec![a.clone(), Term::Never])), "FALSE");
    assert_eq!(term(Term::Any(vec![a, Term::Always])), "TRUE");
    assert_eq!(term(Term::Not(Box::new(Term::Never))), "TRUE");
    assert_eq!(term(Term::All(vec![])), "TRUE");
    assert_eq!(term(Term::Any(vec![])), "FALSE");
}

#[test]
fn not_of_else_deny_is_true_when_the_setting_is_absent() {
    // The app: `negate(Never)` is `Always`. `NOT (FALSE)` is TRUE.
    assert_eq!(
        term(Term::Not(Box::new(cmp(
            AuthorId,
            Op::Eq,
            attr("user_id", Missing::Deny)
        )))),
        format!(r#"NOT ({S_USER} IS NOT NULL AND "author_id" = {S_USER})"#)
    );
}

fn membership(negated: bool, missing: Missing) -> Term<PostColumn> {
    Term::Exists(Box::new(Exists {
        local: Id,
        table: Ident::must("post_members"),
        foreign: Ident::must("post_id"),
        filter: vec![
            (Ident::must("user_id"), Op::Eq, attr("user_id", missing)),
            (Ident::must("kind"), Op::Ne, lit("blocked")),
        ],
        negated,
    }))
}

#[test]
fn exists_is_a_subquery_and_its_deny_test_stays_outside() {
    assert_eq!(
        term(membership(false, Missing::Deny)),
        format!(
            r#"{S_USER} IS NOT NULL AND EXISTS(SELECT 1 FROM "post_members" WHERE "post_members"."post_id" = "posts"."id" AND "post_members"."user_id" = {S_USER} AND "post_members"."kind" <> 'blocked')"#
        )
    );
    // Negated, an absent `else deny` value is still FALSE, as
    // `Exists::resolve` returns `Never` before it negates.
    assert_eq!(
        term(membership(true, Missing::Deny)),
        format!(
            r#"{S_USER} IS NOT NULL AND (NOT EXISTS(SELECT 1 FROM "post_members" WHERE "post_members"."post_id" = "posts"."id" AND "post_members"."user_id" = {S_USER} AND "post_members"."kind" <> 'blocked'))"#
        )
    );
    assert_eq!(
        term(membership(false, Missing::Null)),
        format!(
            r#"EXISTS(SELECT 1 FROM "post_members" WHERE "post_members"."post_id" = "posts"."id" AND "post_members"."user_id" = {S_USER} AND "post_members"."kind" <> 'blocked')"#
        )
    );
}

/// docs/policy-rls-bridge.md §0.
fn list_rule() -> Rule<PostColumn> {
    Rule::new(Term::Any(vec![
        cmp(Status, Op::Eq, lit("published")),
        cmp(AuthorId, Op::Eq, attr("user_id", Missing::Deny)),
    ]))
    .arm(Guard::Attr("banned"), Term::Never)
    .arm(Guard::Attr("admin"), Term::Always)
}

#[test]
fn a_guard_chain_is_boolean_with_the_not_null_prefix() {
    // The `banned => never` arm drops out; its guard stays in the later
    // arms, negated, and in the prefix.
    assert_eq!(
        sql(list_rule()),
        format!(
            r#"{S_BANNED} IS NOT NULL AND {S_ADMIN} IS NOT NULL AND (((NOT {S_BANNED}) AND {S_ADMIN}) OR ((NOT {S_BANNED}) AND (NOT {S_ADMIN}) AND ("status" = 'published' OR ({S_USER} IS NOT NULL AND "author_id" = {S_USER}))))"#
        )
    );
}

#[test]
fn guards_compare_negate_and_combine() {
    let rule = Rule::never().arm(
        Guard::Any(vec![
            Guard::Eq("role", Value::from("editor")),
            Guard::All(vec![
                Guard::Attr("admin"),
                Guard::Not(Box::new(Guard::Attr("banned"))),
            ]),
        ]),
        cmp(Status, Op::Ne, lit("secret")),
    );
    assert_eq!(
        sql(rule),
        format!(
            r#"{S_ROLE} IS NOT NULL AND {S_ADMIN} IS NOT NULL AND {S_BANNED} IS NOT NULL AND (({S_ROLE} = 'editor' OR ({S_ADMIN} AND (NOT {S_BANNED}))) AND "status" <> 'secret')"#
        )
    );
}

#[test]
fn a_rule_that_is_never_in_every_arm_is_false() {
    let rule = Rule::never()
        .arm(Guard::Attr("admin"), Term::Never)
        .arm(Guard::Attr("banned"), Term::All(vec![Term::Never]));
    assert_eq!(sql(rule), "FALSE");
}

#[test]
fn literals_of_every_kind_are_escaped_by_sea_query() {
    let t = |v: Value| term(cmp(Title, Op::Eq, Operand::Value(v)));
    assert_eq!(t(Value::Bool(true)), r#""title" = TRUE"#);
    assert_eq!(t(Value::Float(1.5)), r#""title" = 1.5"#);
    assert_eq!(
        t(Value::Uuid([0xab; 16])),
        r#""title" = 'abababab-abab-abab-abab-abababababab'"#
    );
    assert_eq!(
        t(Value::Timestamptz(0)),
        r#""title" = '1970-01-01 00:00:00.000000 +00:00'"#
    );
    assert_eq!(t(Value::Bytes(vec![1, 2])), r#""title" = '\x0102'"#);
}

#[test]
fn shapes_the_database_cannot_follow_are_refused_with_help() {
    let one = |t: Term<PostColumn>| refused(Rule::new(t));
    let e = one(cmp(AuthorId, Op::Eq, attr("user_id", Missing::Allow)));
    assert!(
        e.contains("help: put the fact in a bool #[attr] field and guard on it"),
        "{e}"
    );
    for op in [Op::Is, Op::IsNot] {
        let e = one(cmp(AuthorId, op, attr("user_id", Missing::Deny)));
        assert!(
            e.contains("help: use == with else deny; IS NOT DISTINCT FROM matches NULL to a missing setting"),
            "{e}"
        );
    }
    let e = one(membership(false, Missing::Allow));
    assert!(e.contains("help: put the fact"), "{e}");
    let e = one(Term::Not(Box::new(membership(false, Missing::Deny))));
    assert!(e.contains("help: negate the membership itself"), "{e}");
    let e = one(cmp(AuthorId, Op::Eq, attr("nobody", Missing::Deny)));
    assert!(
        e.contains("help: add #[attr] to the subject's field `nobody`"),
        "{e}"
    );
    let e = refused(Rule::never().arm(Guard::Attr("role"), Term::Always));
    assert!(e.contains("not a bool") && e.contains("help: "), "{e}");
    let e = refused(Rule::app_only("calls an HTTP service"));
    assert!(
        e.contains("app-only (calls an HTTP service)") && e.contains("help: "),
        "{e}"
    );
}

fn app_user() -> Role {
    Role::new("app_user").member("app_login")
}

fn schema(policies: brocanth_rls::RulePolicies) -> Schema {
    Schema::new()
        .role(app_user())
        .role(Role::existing("app_login"))
        .policies(policies)
}

#[test]
fn from_rules_writes_each_command_its_grant_and_no_more() {
    let sql = schema(Policy::from_rules::<PostPolicy>(&app_user()).sequence("posts_id_seq"))
        .create_sql()
        .unwrap();
    golden("from_rules.sql", &sql);
    assert!(sql.contains(r#"CREATE POLICY "posts_app_user_select" ON "posts" AS PERMISSIVE FOR SELECT TO "app_user" USING ("#));
    assert!(sql.contains(r#"FOR INSERT TO "app_user" WITH CHECK ("#));
    assert!(sql.contains(r#"FOR UPDATE TO "app_user" USING ("#));
    assert!(!sql.contains("posts_app_user_delete"), "{sql}");
    assert!(
        sql.contains(r#"GRANT SELECT, INSERT, UPDATE ON "posts" TO "app_user""#),
        "{sql}"
    );
    assert!(
        sql.contains(r#"GRANT USAGE ON SEQUENCE "posts_id_seq" TO "app_user""#),
        "{sql}"
    );
    assert!(
        sql.contains(r#"ALTER TABLE "posts" FORCE ROW LEVEL SECURITY"#),
        "{sql}"
    );
    // The policy's own resolve is unchanged by the bridge.
    let who = Visitor {
        admin: true,
        ..Visitor::default()
    };
    assert_eq!(PostPolicy.list(&who), brocanth_policy::Pred::Always);
}

#[test]
fn a_db_policy_from_the_macro_writes_the_same_sql_as_its_rules_by_hand() {
    assert_eq!(PostDb::rules(), post_rules());
    let by_hand = schema(Policy::from_rules::<PostPolicy>(&app_user()));
    let by_macro = schema(Policy::from_rules::<PostDb>(&app_user()));
    assert_eq!(
        by_macro.create_sql().unwrap(),
        by_hand.create_sql().unwrap()
    );
}

#[test]
fn select_is_list_alone_without_a_view_and_update_check_defaults_to_update() {
    pub struct Plain;
    db_policy!(Plain, {
        let mut rules = Rules::deny_all();
        rules.list = Rule::new(cmp(Status, Op::Eq, lit("published")));
        rules.update = Rule::new(cmp(Status, Op::Eq, lit("draft")));
        rules
    });
    let snapshot = schema(Policy::from_rules::<Plain>(&app_user()))
        .snapshot()
        .unwrap();
    let posts = &snapshot.tables["posts"];
    let select = &posts.policies["posts_app_user_select"];
    assert_eq!(select.using.as_deref(), Some(r#""status" = 'published'"#));
    let update = &posts.policies["posts_app_user_update"];
    assert_eq!(update.using.as_deref(), Some(r#""status" = 'draft'"#));
    assert_eq!(update.with_check, update.using);
    assert_eq!(posts.policies.len(), 2);
    assert!(posts.force);
    // No insert grant, so no sequence usage.
    assert!(posts.sequence.is_none());
}

#[test]
fn a_policy_that_denies_everything_writes_no_policy_and_no_grant() {
    pub struct Closed;
    db_policy!(Closed, Rules::deny_all());
    let snapshot = schema(Policy::from_rules::<Closed>(&app_user()).sequence("posts_id_seq"))
        .snapshot()
        .unwrap();
    let posts = &snapshot.tables["posts"];
    assert!(posts.policies.is_empty());
    assert!(posts.grants.is_empty());
    assert!(posts.sequence.as_ref().unwrap().usage.is_empty());
}

pub struct AppOnly;
db_policy!(AppOnly, {
    let mut rules = post_rules();
    rules.update = Rule::app_only("the editor service decides");
    rules
});

#[test]
fn an_app_only_rule_is_refused_until_manual_covers_its_command() {
    let e = schema(Policy::from_rules::<AppOnly>(&app_user()))
        .snapshot()
        .expect_err("refused")
        .to_string();
    assert!(
        e.contains(
            "help: write a SqlExpr for posts UPDATE in src/plugins/rls/mod.rs with .manual(...)"
        ),
        "{e}"
    );
    assert!(e.contains("the editor service decides"), "{e}");
    // `manual` on another command does not cover it.
    let other =
        Policy::from_rules::<AppOnly>(&app_user()).manual(Command::Delete, SqlExpr::never());
    assert!(schema(other).snapshot().is_err());

    let floor = SqlExpr::col_named("status").eq(SqlExpr::val("draft"));
    let snapshot =
        schema(Policy::from_rules::<AppOnly>(&app_user()).manual(Command::Update, floor))
            .snapshot()
            .unwrap();
    let update = &snapshot.tables["posts"].policies["posts_app_user_update"];
    assert_eq!(update.using.as_deref(), Some(r#""status" = 'draft'"#));
    assert_eq!(update.with_check, update.using);
}

#[test]
fn a_rule_the_database_cannot_follow_fails_the_schema_and_manual_replaces_it() {
    pub struct Open;
    db_policy!(Open, {
        let mut rules = Rules::deny_all();
        rules.list = Rule::new(cmp(AuthorId, Op::Eq, attr("user_id", Missing::Allow)));
        rules
    });
    let e = schema(Policy::from_rules::<Open>(&app_user()))
        .snapshot()
        .expect_err("refused")
        .to_string();
    assert!(e.starts_with("posts SELECT: "), "{e}");
    assert!(e.contains(".manual(...)"), "{e}");
    let fixed = Policy::from_rules::<Open>(&app_user()).manual(Command::Select, SqlExpr::always());
    schema(fixed).snapshot().unwrap();
    let all = Policy::from_rules::<PostPolicy>(&app_user()).manual(Command::All, SqlExpr::always());
    let e = schema(all).snapshot().expect_err("refused").to_string();
    assert!(e.contains("help: name one command"), "{e}");
}

#[test]
fn attrs_is_the_derives_list() {
    let keys: Vec<&str> = Visitor::ATTRS.iter().map(|a: &Attr| a.key).collect();
    assert_eq!(keys, ["user_id", "tenant_id", "admin", "banned", "role"]);
}
