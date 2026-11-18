//! `#[db]` changes where a rule lives, not what it means.
//!
//! docs/policy-rls-bridge.md, decision 2: with `#[db]`, `policy!` emits
//! `DbPolicy::rules()` and implements `Policy` by resolving those rules with
//! the subject's attributes. Each policy here is written twice, with the
//! same syntax: once without `#[db]` (the Rust expansion) and once with it.
//! For every subject and every command, both give the same `Pred`, the same
//! SQL and binds, and the same `eval` on each sample row.
//!
//! The `#[db]` policies with an `eval { … }` clause also run the agreement
//! test on PostgreSQL. Without `BROCANTH_TEST_DATABASE_URL`, it prints
//! `!! SKIPPED` and passes.

#[allow(dead_code)]
mod postgres;

use brocanth_policy::{
    Bound, DbPolicy, Dialect, EntityPred, Ident, MapRow, Membership, Op, Policy, Pred, Rule,
    Subject, Term, Truth, Value,
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
    pub team: Option<Uuid>,
}

impl Visitor {
    fn user(id: i64) -> Self {
        Visitor {
            user_id: Some(id),
            ..Visitor::default()
        }
    }

    fn admin() -> Self {
        Visitor {
            user_id: Some(0),
            admin: true,
            ..Visitor::default()
        }
    }

    fn banned(id: i64) -> Self {
        Visitor {
            banned: true,
            ..Visitor::user(id)
        }
    }

    fn in_team(id: i64, team: u128) -> Self {
        Visitor {
            team: Some(Uuid::from_u128(team)),
            ..Visitor::user(id)
        }
    }
}

fn visitors() -> Vec<Visitor> {
    vec![
        Visitor::default(),
        Visitor::user(1),
        Visitor::user(2),
        Visitor::admin(),
        Visitor::banned(1),
        Visitor::in_team(1, 9),
        Visitor {
            team: Some(Uuid::from_u128(9)),
            ..Visitor::default()
        },
    ]
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
        score: i64,
    }

    pub entity PostTeam in "post_teams" {
        #[key] post_id: i64,
        team_id: Option<Uuid>,
    }

    pub entity Block in "blocks" {
        #[key] blocked_id: i64,
        blocker_id: i64,
    }

    /// docs/policy-rls-bridge.md §0, in Rust.
    pub policy PostApp(who: &Visitor) for Post {
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
        writable {
            who.admin => except [id],
            only [status],
        }
    }

    /// docs/policy-rls-bridge.md §0, as data.
    #[db]
    pub policy PostDb(who: &Visitor) for Post {
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
        writable {
            who.admin => except [id],
            only [status],
        }

        eval {
            subjects: postgres::or_skip([
                Visitor::default(),
                Visitor::user(1),
                Visitor::user(2),
                Visitor::admin(),
                Visitor::banned(1),
            ]),
            database: postgres::posts,
            cases: 32,
        }
    }

    /// A status change: an author may edit a draft, and publish it. The
    /// application checks the stored row (`update`) and the mask; the
    /// database checks the new row too (`update_check`, decision 8).
    pub policy ArticleApp(who: &Visitor) for Post {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        writable {
            only [status],
        }
    }

    #[db]
    pub policy ArticleDb(who: &Visitor) for Post {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        update_check {
            who.admin => always,
            author_id == who.user_id else deny && status in ["draft", "published"],
        }
        writable {
            only [status],
        }

        eval {
            subjects: postgres::or_skip([
                Visitor::default(),
                Visitor::user(1),
                Visitor::admin(),
            ]),
            database: postgres::posts,
            cases: 32,
        }
    }

    /// Membership by hand: the form `exists` replaces.
    pub policy TeamsHand(who: &Visitor) for Post {
        list: {
            match Membership::new(PostColumn::Id, Ident::must("post_teams"), Ident::must("post_id"))
                .filter_by(Ident::must("team_id"), Op::Eq, Bound::or_deny(who.team))
            {
                Some(m) => m.into_pred(),
                None => Pred::Never,
            }
        },
        view: {
            match Membership::new(PostColumn::AuthorId, Ident::must("blocks"), Ident::must("blocked_id"))
                .filter_by(Ident::must("blocker_id"), Op::Eq, Bound::or_deny(who.user_id))
            {
                Some(m) => m.negate().into_pred(),
                None => Pred::Never,
            }
        },
    }

    /// The same, with `exists`, in Rust.
    pub policy TeamsApp(who: &Visitor) for Post {
        list: exists PostTeam(post_id == id, team_id == who.team else deny),
        /// Not the posts of an author who blocked you.
        view: !exists Block(blocked_id == author_id, blocker_id == who.user_id else deny),
        create: exists PostTeam(post_id == id, team_id is null) || score > 2,
        update: exists PostTeam(post_id == id, team_id == who.team else null),
        delete: !(exists PostTeam(post_id == id, team_id is not null) && status == "x"),
    }

    /// The same, with `exists`, as data.
    #[db]
    pub policy TeamsDb(who: &Visitor) for Post {
        list: exists PostTeam(post_id == id, team_id == who.team else deny),
        /// Not the posts of an author who blocked you.
        view: !exists Block(blocked_id == author_id, blocker_id == who.user_id else deny),
        create: exists PostTeam(post_id == id, team_id is null) || score > 2,
        update: exists PostTeam(post_id == id, team_id == who.team else null),
        delete: !(exists PostTeam(post_id == id, team_id is not null) && status == "x"),
    }

    /// One rule the database cannot check.
    #[db]
    pub policy Clocked(who: &Visitor) for Post {
        list: status == "published",
        /// Admins only, on weekdays (the clock is left out here).
        #[app_only("compares with the clock")]
        delete: { if who.admin { Pred::Always } else { Pred::Never } },
    }
}

fn rows() -> Vec<MapRow<PostColumn>> {
    let row = |id: i64, author: Option<i64>, status: &str, score: i64| {
        let row = MapRow::new()
            .with(PostColumn::Id, id)
            .with(PostColumn::Status, status)
            .with(PostColumn::Score, score);
        match author {
            Some(a) => row.with(PostColumn::AuthorId, a),
            None => row.with_null(PostColumn::AuthorId),
        }
    };
    vec![
        row(1, Some(1), "draft", 0),
        row(2, Some(2), "published", 5),
        row(3, None, "published", 1),
        row(4, Some(1), "published", 3),
        row(5, None, "x", 0),
    ]
}

type Method<P> = fn(&P, &Visitor) -> EntityPred<Post>;

fn commands<P: Policy<Entity = Post, Subject = Visitor>>() -> [(&'static str, Method<P>); 5] {
    [
        ("list", P::list),
        ("view", P::view),
        ("create", P::create),
        ("update", P::update),
        ("delete", P::delete),
    ]
}

/// For every visitor and command: the same `Pred`, SQL, binds, and `eval`
/// on rows the predicate can evaluate.
fn assert_same<A, B>(app: &A, db: &B)
where
    A: Policy<Entity = Post, Subject = Visitor>,
    B: Policy<Entity = Post, Subject = Visitor>,
{
    for who in visitors() {
        for ((name, a), (_, b)) in commands::<A>().into_iter().zip(commands::<B>()) {
            let a = a(app, &who);
            let b = b(db, &who);
            assert_eq!(a, b, "`{name}` for {who:?}");
            let sa = a.condition::<Post>(Dialect::Postgres).expect("compiles");
            let sb = b.condition::<Post>(Dialect::Postgres).expect("compiles");
            assert_eq!(sa.sql(), sb.sql(), "`{name}` SQL for {who:?}");
            assert_eq!(sa.binds(), sb.binds(), "`{name}` binds for {who:?}");
            for row in rows() {
                // `eval` cannot run `EXISTS`: both sides then fail alike.
                assert_eq!(
                    a.eval(&row).ok(),
                    b.eval(&row).ok(),
                    "`{name}` eval for {who:?}"
                );
            }
        }
    }
}

#[test]
fn the_doc_example_is_the_same_with_and_without_db() {
    assert_same(&PostApp, &PostDb);
    // The masks stay Rust, unchanged.
    for who in visitors() {
        assert_eq!(PostApp.writable(&who), PostDb.writable(&who));
    }
}

#[test]
fn a_status_change_is_the_same_with_and_without_db() {
    assert_same(&ArticleApp, &ArticleDb);
    // `update_check` is data only: the application does not read it.
    assert!(ArticleApp
        .writable(&Visitor::user(1))
        .permits(PostColumn::Status));
}

#[test]
fn update_check_lets_an_author_publish_and_nothing_else() {
    let rules = ArticleDb::rules();
    assert!(rules.update_check.is_some());
    let author = Visitor::user(1).attributes();
    let update = rules.update.resolve(&author);
    let check = rules.update_check().resolve(&author);
    let row = |author: i64, status: &str| {
        MapRow::new()
            .with(PostColumn::Id, 1i64)
            .with(PostColumn::AuthorId, author)
            .with(PostColumn::Status, status)
            .with(PostColumn::Score, 0i64)
    };
    let eval =
        |pred: &EntityPred<Post>, row: &MapRow<PostColumn>| pred.eval(row).expect("evaluates");
    // The stored draft may change; the new row may be the published post.
    assert_eq!(eval(&update, &row(1, "draft")), Truth::True);
    assert_eq!(eval(&check, &row(1, "published")), Truth::True);
    // A published post is not editable, and the new row may not leave the
    // author or take another status.
    assert_eq!(eval(&update, &row(1, "published")), Truth::False);
    assert_eq!(eval(&check, &row(2, "published")), Truth::False);
    assert_eq!(eval(&check, &row(1, "archived")), Truth::False);
    // Anonymous: never; admin: always.
    assert!(rules
        .update_check()
        .resolve(&Visitor::default().attributes())
        .is_never());
    assert!(rules
        .update_check()
        .resolve(&Visitor::admin().attributes())
        .is_always());
    // A policy without `update_check` reads `update`.
    let post = PostDb::rules();
    assert!(post.update_check.is_none());
    assert_eq!(post.update_check(), &post.update);
}

#[test]
fn exists_is_the_hand_written_membership() {
    assert_same(&TeamsApp, &TeamsDb);
    for who in visitors() {
        assert_eq!(
            TeamsHand.list(&who),
            TeamsApp.list(&who),
            "list for {who:?}"
        );
        assert_eq!(
            TeamsHand.view(&who),
            TeamsApp.view(&who),
            "view for {who:?}"
        );
    }
    let member = TeamsDb.list(&Visitor::in_team(1, 9));
    let Pred::Exists(membership) = &member else {
        panic!("expected a membership, got {member:?}");
    };
    assert_eq!(membership.table, Ident::must("post_teams"));
    assert_eq!(membership.foreign, Ident::must("post_id"));
    assert_eq!(
        membership.filter,
        [(
            Ident::must("team_id"),
            Op::Eq,
            Value::from(Uuid::from_u128(9))
        )]
    );
    let sql = member
        .condition::<Post>(Dialect::Postgres)
        .expect("compiles");
    assert!(sql.sql().contains("EXISTS"), "{}", sql.sql());
}

#[test]
fn not_exists_with_an_absent_else_deny_value_is_never() {
    // `else deny` makes the term that holds it `never`, and `!exists` is
    // one term: an anonymous visitor sees nothing, not every post.
    let anonymous = Visitor::default();
    assert!(TeamsApp.view(&anonymous).is_never());
    assert!(TeamsDb.view(&anonymous).is_never());
    assert!(TeamsHand.view(&anonymous).is_never());
    let view = TeamsDb::rules().view.expect("written");
    let Term::Exists(exists) = &view.fallback else {
        panic!("expected exists, got {:?}", view.fallback);
    };
    assert!(exists.negated);
    // A user: `NOT EXISTS`.
    let Pred::Exists(m) = TeamsDb.view(&Visitor::user(1)) else {
        panic!("expected a membership");
    };
    assert!(m.negated);
    // `else null` keeps the filter, as `NULL`.
    let Pred::Exists(m) = TeamsDb.update(&anonymous) else {
        panic!("expected a membership");
    };
    assert_eq!(m.filter, [(Ident::must("team_id"), Op::Eq, Value::Null)]);
}

#[test]
fn an_app_only_rule_stays_rust_and_is_marked_in_the_rules() {
    let rules = Clocked::rules();
    assert_eq!(rules.delete, Rule::app_only("compares with the clock"));
    assert_eq!(rules.delete.app_only, Some("compares with the clock"));
    assert!(rules
        .delete
        .resolve(&Visitor::admin().attributes())
        .is_never());
    // The application runs the Rust code.
    assert!(Clocked.delete(&Visitor::admin()).is_always());
    assert!(Clocked.delete(&Visitor::user(1)).is_never());
    // The other rules are data as usual.
    assert!(rules.list.app_only.is_none());
    assert_eq!(
        Clocked.list(&Visitor::default()),
        Pred::eq(PostColumn::Status, "published")
    );
}

#[test]
fn a_mask_without_a_guard_is_a_fixed_column_list_and_a_guarded_one_is_not() {
    use brocanth_policy::{FieldMask, MaskRule};
    // `only [status]`, the same for every subject.
    let article = ArticleDb::rules();
    assert_eq!(
        article.writable,
        Some(MaskRule::Fixed(FieldMask::only([PostColumn::Status])))
    );
    assert_eq!(article.writable_on_create, None);
    assert_eq!(article.writable_on_create(), article.writable.as_ref());
    for who in visitors() {
        assert_eq!(
            Some(MaskRule::Fixed(ArticleDb.writable(&who))),
            article.writable
        );
    }
    // `who.admin => except [id]`: it depends on the subject.
    assert_eq!(PostDb::rules().writable, Some(MaskRule::PerSubject));
    // No mask written: nothing in the rules.
    assert_eq!(TeamsDb::rules().writable, None);
}

brocanth_policy::policy! {
    pub entity Note in "notes" {
        #[key] id: i64,
        owner_id: i64,
        body: String,
    }

    #[db]
    pub policy NoteDb(who: &Visitor) for Note {
        list: owner_id == who.user_id else deny,
        writable: except [id, owner_id],
        writable_on_create: {
            if who.admin {
                brocanth_policy::FieldMask::all()
            } else {
                brocanth_policy::FieldMask::only([NoteColumn::Body])
            }
        },
    }
}

#[test]
fn an_except_mask_and_a_block_mask_are_data_too() {
    use brocanth_policy::{FieldMask, MaskRule};
    let rules = NoteDb::rules();
    assert_eq!(
        rules.writable,
        Some(MaskRule::Fixed(FieldMask::except([
            NoteColumn::Id,
            NoteColumn::OwnerId
        ])))
    );
    assert_eq!(
        rules.writable.as_ref().and_then(MaskRule::columns),
        Some(vec![NoteColumn::Body])
    );
    assert_eq!(rules.writable_on_create, Some(MaskRule::PerSubject));
    // The application still runs the block.
    assert!(NoteDb
        .writable_on_create(&Visitor::admin())
        .permits(NoteColumn::OwnerId));
}
