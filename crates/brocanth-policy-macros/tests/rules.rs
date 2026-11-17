//! `Rules` resolved with a subject's attributes is today's `policy!`
//! expansion (docs/policy-rls-bridge.md, decision 2).
//!
//! Each policy is written twice: with `policy!`, as today, and as a
//! `Rules<Post>` tree by hand. For every subject and every command,
//! `rule.resolve(&who.attributes())` must be the same `Pred`, with the same
//! SQL and binds, as the `Policy` method. The `#[db]` twins then show that
//! `policy!` with `#[db]` emits exactly the hand-built tree.

use brocanth_policy::{
    Attr, AttrKind, Attributes, DbPolicy, Dialect, EntityPred, Exists, Guard, Ident, Missing, Op,
    Operand, Policy, Pred, Rule, Rules, Subject, Term, Value,
};
use uuid::Uuid;

#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub admin: bool,
    #[attr]
    pub banned: bool,
    #[attr]
    pub role: String,
    #[attr]
    pub team: Option<Uuid>,
    /// Not an attribute: no rule reads it, and it never reaches the
    /// database.
    #[allow(dead_code)]
    pub display_name: String,
}

fn visitors() -> Vec<Visitor> {
    let v = |user_id, admin, banned, role: &str, team| Visitor {
        user_id,
        admin,
        banned,
        role: role.to_owned(),
        team,
        display_name: "x".to_owned(),
    };
    vec![
        v(None, false, false, "", None),
        v(Some(1), false, false, "editor", None),
        v(Some(2), false, false, "", Some(Uuid::from_u128(9))),
        v(Some(0), true, false, "", None),
        v(None, true, false, "editor", None),
        v(Some(7), false, true, "editor", None),
        v(Some(7), true, true, "", Some(Uuid::from_u128(1))),
    ]
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
        title: String,
    }

    /// The link table of `Teams`.
    pub entity PostTeam in "post_teams" {
        #[key] post_id: i64,
        team_id: Option<Uuid>,
    }

    /// docs/policy-rls-bridge.md §0, and docs/policy.md's example.
    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny,
        }
        delete {
            who.admin => always,
        }
    }

    /// Every other leaf shape and absence mode.
    pub policy Shapes(who: &Visitor) for Post {
        list: !(status == "draft") && (author_id is not null || title in ["a", "b"]),
        view: status not in ["x", "y"] && author_id is null,
        create: author_id is who.user_id else null || author_id is not who.user_id else allow,
        update: author_id == who.user_id else null && status != "gone",
        delete: author_id < 10 && author_id <= 9 && author_id > 0 && author_id >= 1 && author_id != 5,
    }

    /// Guard chains: `!`, `&&`, `||`, `== lit`, and an empty `in`.
    pub policy Guards(who: &Visitor) for Post {
        list {
            who.admin && !who.banned => always,
            who.role == "editor" || who.admin => status != "secret",
            !who.banned => status == "published" || author_id == who.user_id else deny,
            never,
        }
        view {
            !(who.role == "editor") => title in [] as [&str; 0],
            title not in [] as [&str; 0],
        }
        update {
            who.banned => author_id == who.user_id else allow,
        }
    }

    /// `PostPolicy` with `#[db]`.
    #[db]
    pub policy PostPolicyDb(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny,
        }
        delete {
            who.admin => always,
        }
    }

    /// `Guards` with `#[db]`, without its `update` (`else allow` is refused).
    #[db]
    pub policy GuardsDb(who: &Visitor) for Post {
        list {
            who.admin && !who.banned => always,
            who.role == "editor" || who.admin => status != "secret",
            !who.banned => status == "published" || author_id == who.user_id else deny,
            never,
        }
        view {
            !(who.role == "editor") => title in [] as [&str; 0],
            title not in [] as [&str; 0],
        }
    }

    /// `Teams` with the `exists` syntax and `#[db]`.
    #[db]
    pub policy TeamsDb(who: &Visitor) for Post {
        list: exists PostTeam(post_id == id, team_id == who.team else deny),
    }

    /// Membership, by hand: the form `exists` replaces.
    pub policy Teams(who: &Visitor) for Post {
        list: {
            match brocanth_policy::Membership::new(
                PostColumn::Id,
                brocanth_policy::Ident::must("post_teams"),
                brocanth_policy::Ident::must("post_id"),
            )
            .filter_by(
                brocanth_policy::Ident::must("team_id"),
                brocanth_policy::Op::Eq,
                brocanth_policy::Bound::or_deny(who.team),
            ) {
                Some(m) => m.into_pred(),
                None => brocanth_policy::Pred::Never,
            }
        },
    }
}

fn attr(key: &'static str, missing: Missing) -> Operand {
    Operand::Attr { key, missing }
}

fn lit(v: impl Into<Value>) -> Operand {
    Operand::Value(v.into())
}

fn cmp(column: PostColumn, op: Op, operand: Operand) -> Term<PostColumn> {
    Term::Cmp {
        column,
        op,
        operand,
    }
}

fn post_rules() -> Rules<Post> {
    let owner = cmp(PostColumn::AuthorId, Op::Eq, attr("user_id", Missing::Deny));
    let mut rules = Rules::deny_all();
    rules.list = Rule::new(Term::Any(vec![
        cmp(PostColumn::Status, Op::Eq, lit("published")),
        owner.clone(),
    ]))
    .arm(Guard::Attr("banned"), Term::Never)
    .arm(Guard::Attr("admin"), Term::Always);
    rules.create = Rule::new(owner.clone());
    rules.update = Rule::new(owner).arm(Guard::Attr("admin"), Term::Always);
    rules.delete = Rule::never().arm(Guard::Attr("admin"), Term::Always);
    rules
}

fn shapes_rules() -> Rules<Post> {
    use PostColumn::*;
    let mut rules = Rules::deny_all();
    rules.list = Rule::new(Term::All(vec![
        Term::Not(Box::new(cmp(Status, Op::Eq, lit("draft")))),
        Term::Any(vec![
            cmp(AuthorId, Op::IsNot, Operand::Value(Value::Null)),
            Term::In {
                column: Title,
                values: vec!["a".into(), "b".into()],
                negated: false,
            },
        ]),
    ]));
    rules.view = Some(Rule::new(Term::All(vec![
        Term::In {
            column: Status,
            values: vec!["x".into(), "y".into()],
            negated: true,
        },
        cmp(AuthorId, Op::Is, Operand::Value(Value::Null)),
    ])));
    rules.create = Rule::new(Term::Any(vec![
        cmp(AuthorId, Op::Is, attr("user_id", Missing::Null)),
        cmp(AuthorId, Op::IsNot, attr("user_id", Missing::Allow)),
    ]));
    rules.update = Rule::new(Term::All(vec![
        cmp(AuthorId, Op::Eq, attr("user_id", Missing::Null)),
        cmp(Status, Op::Ne, lit("gone")),
    ]));
    rules.delete = Rule::new(Term::All(vec![
        cmp(AuthorId, Op::Lt, lit(10i64)),
        cmp(AuthorId, Op::Le, lit(9i64)),
        cmp(AuthorId, Op::Gt, lit(0i64)),
        cmp(AuthorId, Op::Ge, lit(1i64)),
        cmp(AuthorId, Op::Ne, lit(5i64)),
    ]));
    rules
}

fn guards_rules() -> Rules<Post> {
    use PostColumn::*;
    let mut rules = Rules::deny_all();
    rules.list = Rule::never()
        .arm(
            Guard::All(vec![
                Guard::Attr("admin"),
                Guard::Not(Box::new(Guard::Attr("banned"))),
            ]),
            Term::Always,
        )
        .arm(
            Guard::Any(vec![
                Guard::Eq("role", Value::from("editor")),
                Guard::Attr("admin"),
            ]),
            cmp(Status, Op::Ne, lit("secret")),
        )
        .arm(
            Guard::Not(Box::new(Guard::Attr("banned"))),
            Term::Any(vec![
                cmp(Status, Op::Eq, lit("published")),
                cmp(AuthorId, Op::Eq, attr("user_id", Missing::Deny)),
            ]),
        );
    rules.view = Some(
        Rule::new(Term::In {
            column: Title,
            values: vec![],
            negated: true,
        })
        .arm(
            Guard::Not(Box::new(Guard::Eq("role", Value::from("editor")))),
            Term::In {
                column: Title,
                values: vec![],
                negated: false,
            },
        ),
    );
    rules.update = Rule::never().arm(
        Guard::Attr("banned"),
        cmp(AuthorId, Op::Eq, attr("user_id", Missing::Allow)),
    );
    rules
}

fn teams_rules() -> Rules<Post> {
    let mut rules = Rules::deny_all();
    rules.list = Rule::new(Term::Exists(Box::new(Exists {
        local: PostColumn::Id,
        table: Ident::must("post_teams"),
        foreign: Ident::must("post_id"),
        filter: vec![(Ident::must("team_id"), Op::Eq, attr("team", Missing::Deny))],
        negated: false,
    })));
    rules
}

/// Every command of `policy`, for every visitor: the same `Pred`, SQL and
/// binds from both sides.
fn assert_same<P>(policy: &P, rules: &Rules<Post>)
where
    P: Policy<Entity = Post, Subject = Visitor>,
{
    type Method<P> = fn(&P, &Visitor) -> EntityPred<Post>;
    let commands: [(&str, Method<P>, &Rule<PostColumn>); 5] = [
        ("list", P::list, &rules.list),
        ("view", P::view, rules.view()),
        ("create", P::create, &rules.create),
        ("update", P::update, &rules.update),
        ("delete", P::delete, &rules.delete),
    ];
    for who in visitors() {
        let attrs = who.attributes();
        for (name, method, rule) in &commands {
            let expected = method(policy, &who);
            let resolved = rule.resolve(&attrs);
            assert_eq!(resolved, expected, "`{name}` for {who:?}");
            let a = resolved.condition::<Post>(Dialect::Postgres);
            let b = expected.condition::<Post>(Dialect::Postgres);
            match (a, b) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(a.sql(), b.sql(), "`{name}` SQL for {who:?}");
                    assert_eq!(a.binds(), b.binds(), "`{name}` binds for {who:?}");
                }
                (a, b) => assert_eq!(a.is_ok(), b.is_ok(), "`{name}` for {who:?}"),
            }
        }
    }
}

#[test]
fn the_doc_example_resolves_to_the_policy_expansion() {
    assert_same(&PostPolicy, &post_rules());
}

#[test]
fn every_leaf_and_absence_mode_resolves_to_the_policy_expansion() {
    assert_same(&Shapes, &shapes_rules());
}

#[test]
fn guard_chains_resolve_to_the_policy_expansion() {
    assert_same(&Guards, &guards_rules());
}

#[test]
fn membership_resolves_to_the_hand_written_exists() {
    assert_same(&Teams, &teams_rules());
}

#[test]
fn db_emits_the_hand_built_tree() {
    assert_eq!(PostPolicyDb::rules(), post_rules());
    assert_eq!(TeamsDb::rules(), teams_rules());
    let guards = GuardsDb::rules();
    let expected = guards_rules();
    assert_eq!(guards.list, expected.list);
    assert_eq!(guards.view, expected.view);
    assert_eq!(guards.update, Rule::never());
}

#[test]
fn db_policies_resolve_to_the_hand_written_policy() {
    assert_same(&PostPolicyDb, &post_rules());
    assert_same(&TeamsDb, &teams_rules());
}

#[test]
fn the_folding_shows_what_absence_did() {
    let rules = post_rules();
    let anonymous = Visitor::default().attributes();
    // `else deny` with no user id: only the published half is left.
    assert_eq!(
        rules.list.resolve(&anonymous),
        Pred::eq(PostColumn::Status, "published")
    );
    assert!(rules.create.resolve(&anonymous).is_never());
    // `else null` keeps a comparison with NULL; `else allow` is `always`.
    let shapes = shapes_rules();
    assert_eq!(
        shapes.update.resolve(&anonymous),
        Pred::all([
            Pred::eq(PostColumn::AuthorId, Value::Null),
            Pred::ne(PostColumn::Status, "gone"),
        ])
    );
    assert!(shapes.create.resolve(&anonymous).is_always());
}

#[test]
fn an_absent_guard_attribute_denies_the_whole_rule() {
    // A subject from the derive always has its `bool` fields. A hand-built
    // set without `banned` reads like an empty setting in the database:
    // the rule is FALSE, even for an admin.
    let rules = post_rules();
    let attrs = Attributes::new()
        .with("admin", Some(Value::Bool(true)))
        .with("user_id", Some(Value::Int(1)));
    assert!(rules.list.resolve(&attrs).is_never());
    // A guard that is not reached still counts: `admin` is the second
    // guard, and `banned` holds.
    let attrs = Attributes::new().with("banned", Some(Value::Bool(true)));
    assert!(rules.list.resolve(&attrs).is_never());
    // A non-boolean in a bare guard has no answer, so the rule fails closed.
    let attrs = Attributes::new()
        .with("banned", Some(Value::Int(0)))
        .with("admin", Some(Value::Bool(false)));
    assert!(rules.list.resolve(&attrs).is_never());
    assert_eq!(rules.list.guard_keys(), ["banned", "admin"]);
}

#[test]
fn view_and_update_check_default_to_list_and_update() {
    let rules = post_rules();
    assert!(rules.view.is_none());
    assert_eq!(rules.view(), &rules.list);
    assert!(rules.update_check.is_none());
    assert_eq!(rules.update_check(), &rules.update);
    let shapes = shapes_rules();
    assert_ne!(shapes.view(), &shapes.list);
}

#[test]
fn the_derive_lists_each_attr_field_by_name_kind_and_absence() {
    assert_eq!(
        <Visitor as Subject>::ATTRS,
        &[
            Attr {
                key: "user_id",
                kind: AttrKind::BigInt,
                optional: true
            },
            Attr {
                key: "admin",
                kind: AttrKind::Boolean,
                optional: false
            },
            Attr {
                key: "banned",
                kind: AttrKind::Boolean,
                optional: false
            },
            Attr {
                key: "role",
                kind: AttrKind::Text,
                optional: false
            },
            Attr {
                key: "team",
                kind: AttrKind::Uuid,
                optional: true
            },
        ]
    );
    let team = Uuid::from_u128(3);
    let who = Visitor {
        user_id: None,
        admin: true,
        banned: false,
        role: "editor".to_owned(),
        team: Some(team),
        display_name: "Ada".to_owned(),
    };
    let attrs = who.attributes();
    assert_eq!(
        attrs.iter().collect::<Vec<_>>(),
        vec![
            ("user_id", None),
            ("admin", Some(&Value::Bool(true))),
            ("banned", Some(&Value::Bool(false))),
            ("role", Some(&Value::from("editor"))),
            ("team", Some(&Value::from(team))),
        ]
    );
    assert_eq!(attrs.get("display_name"), None);
}

/// Every supported type, with its database type. Raw identifiers lose
/// their `r#`.
#[derive(brocanth_policy::Subject)]
#[allow(dead_code)]
struct EveryKind {
    #[attr]
    r#type: i32,
    #[attr]
    big: Option<i64>,
    #[attr]
    flag: Option<bool>,
    #[attr]
    name: Option<String>,
    #[attr]
    id: Uuid,
    #[attr]
    at: chrono::DateTime<chrono::Utc>,
    #[attr]
    maybe_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[test]
fn each_attribute_type_has_its_database_type() {
    let kinds: Vec<(&str, &str, bool)> = <EveryKind as Subject>::ATTRS
        .iter()
        .map(|a| (a.key, a.kind.sql_type(), a.optional))
        .collect();
    assert_eq!(
        kinds,
        [
            ("type", "integer", false),
            ("big", "bigint", true),
            ("flag", "boolean", true),
            ("name", "text", true),
            ("id", "uuid", false),
            ("at", "timestamptz", false),
            ("maybe_at", "timestamptz", true),
        ]
    );
    let at = chrono::DateTime::<chrono::Utc>::from_timestamp_micros(42).expect("in range");
    let attrs = EveryKind {
        r#type: 3,
        big: None,
        flag: Some(true),
        name: None,
        id: Uuid::nil(),
        at,
        maybe_at: None,
    }
    .attributes();
    assert_eq!(attrs.get("type"), Some(&Value::Int(3)));
    assert_eq!(attrs.get("at"), Some(&Value::Timestamptz(42)));
    assert_eq!(attrs.get("id"), Some(&Value::Uuid([0; 16])));
    assert_eq!(attrs.get("maybe_at"), None);
}
