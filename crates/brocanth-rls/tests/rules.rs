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

#[test]
fn an_inserter_writes_any_row_and_reads_none_while_the_main_role_cannot_insert() {
    pub struct Inbox;
    db_policy!(Inbox, {
        let mut rules = Rules::deny_all();
        rules.list = Rule::new(cmp(AuthorId, Op::Eq, attr("user_id", Missing::Deny)));
        rules
    });
    let writer = Role::new("writer").member("app_login");
    let snapshot = schema(
        Policy::from_rules::<Inbox>(&app_user())
            .inserter(writer)
            .sequence("posts_id_seq"),
    )
    .snapshot()
    .unwrap();
    // `Schema::policies` adds the role.
    assert!(snapshot.roles.contains_key("writer"));
    let posts = &snapshot.tables["posts"];
    let insert = &posts.policies["posts_writer_insert"];
    assert_eq!(insert.with_check.as_deref(), Some("TRUE"));
    assert!(insert.using.is_none());
    assert!(!posts.policies.contains_key("posts_app_user_insert"));
    let privileges =
        |role: &str| -> Vec<String> { posts.grants[role].iter().map(|p| p.to_string()).collect() };
    assert_eq!(privileges("writer"), ["INSERT"]);
    assert_eq!(privileges("app_user"), ["SELECT"]);
    // `USAGE` on the sequence for the writer alone: the reader cannot insert.
    let usage: Vec<&str> = posts
        .sequence
        .as_ref()
        .unwrap()
        .usage
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(usage, ["writer"]);
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

/// A policy whose `list` reads `post_members` through `exists`, or not.
fn members_list(reads: bool) -> Rules<common::policy::Post> {
    let mut rules = Rules::deny_all();
    rules.list = Rule::new(if reads {
        Term::Any(vec![
            membership(false, Missing::Deny),
            cmp(Status, Op::Eq, lit("published")),
        ])
    } else {
        cmp(Status, Op::Eq, lit("published"))
    });
    rules
}

#[test]
fn from_rules_grants_select_on_the_link_table_an_exists_reads() {
    pub struct Reads;
    db_policy!(Reads, members_list(true));
    pub struct Plain;
    db_policy!(Plain, members_list(false));

    let with = schema(Policy::from_rules::<Reads>(&app_user()));
    let sql = with.create_sql().unwrap();
    assert!(
        sql.contains(
            "-- create grant SELECT on link table post_members to app_user\nGRANT SELECT ON \"post_members\" TO \"app_user\";"
        ),
        "{sql}"
    );
    // The link table gets no row-level security from the grant.
    assert!(!sql.contains(r#"ALTER TABLE "post_members""#), "{sql}");
    let with = with.snapshot().unwrap();
    assert_eq!(
        with.link_grants["post_members"].iter().collect::<Vec<_>>(),
        ["app_user"]
    );
    assert!(!with.tables.contains_key("post_members"));
    // In `rls.json`, and an older file without the field reads as none.
    let json = with.to_json();
    assert!(json.contains("\"link_grants\""), "{json}");
    let without = schema(Policy::from_rules::<Plain>(&app_user()))
        .snapshot()
        .unwrap();
    assert!(!without.to_json().contains("link_grants"));
    assert!(without.link_grants.is_empty());

    // The rule stops reading it: the diff revokes it, and the way back
    // grants it again.
    let plan = brocanth_rls::diff(&with, &without).unwrap();
    assert!(
        plan.sql()
            .contains(r#"REVOKE SELECT ON "post_members" FROM "app_user";"#),
        "{}",
        plan.sql()
    );
    let back = brocanth_rls::diff(&without, &with).unwrap();
    assert_eq!(
        back.describe(),
        [
            "alter policy posts_app_user_select on posts",
            "create grant SELECT on link table post_members to app_user"
        ]
    );
    assert!(brocanth_rls::diff(&with, &with).unwrap().is_empty());
}

#[test]
fn a_link_table_the_schema_adds_takes_the_select_on_its_own_grants() {
    pub struct Reads;
    db_policy!(Reads, members_list(true));
    let role = app_user();
    let snapshot = schema(Policy::from_rules::<Reads>(&role))
        .table(Table::named("post_members").grant(&role, [brocanth_rls::Privilege::Insert]))
        .snapshot()
        .unwrap();
    assert!(snapshot.link_grants.is_empty());
    let members = &snapshot.tables["post_members"];
    assert_eq!(
        members.grants["app_user"]
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [
            brocanth_rls::Privilege::Select,
            brocanth_rls::Privilege::Insert
        ]
    );

    // A column `SELECT` the developer chose stays as it is.
    let snapshot = schema(Policy::from_rules::<Reads>(&role))
        .table(Table::named("post_members").grant_columns(
            &role,
            brocanth_rls::Privilege::Select,
            ["post_id", "user_id", "kind"],
        ))
        .snapshot()
        .unwrap();
    let members = &snapshot.tables["post_members"];
    assert!(members.grants.is_empty());
    assert!(snapshot.link_grants.is_empty());
}

#[test]
fn a_command_with_no_policy_or_a_manual_expression_reads_no_link_table() {
    // Only `update` reads the link table, and `.manual` replaces it.
    pub struct UpdateReads;
    db_policy!(UpdateReads, {
        let mut rules = Rules::deny_all();
        rules.list = Rule::new(Term::Always);
        rules.update = Rule::new(membership(false, Missing::Deny));
        rules
    });
    let policies = Policy::from_rules::<UpdateReads>(&app_user());
    assert_eq!(policies.link_tables(), ["post_members"]);
    let manual = policies.manual(Command::Update, SqlExpr::always());
    assert!(manual.link_tables().is_empty());
    assert!(schema(manual).snapshot().unwrap().link_grants.is_empty());

    // A link to the policy's own table is the table's own grant.
    pub struct SelfLink;
    db_policy!(SelfLink, {
        let mut rules = Rules::deny_all();
        rules.list = Rule::new(Term::Exists(Box::new(Exists {
            local: AuthorId,
            table: Ident::must("posts"),
            foreign: Ident::must("author_id"),
            filter: vec![],
            negated: false,
        })));
        rules
    });
    assert!(Policy::from_rules::<SelfLink>(&app_user())
        .link_tables()
        .is_empty());
}
