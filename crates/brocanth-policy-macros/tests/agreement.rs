//! `eval { … }` generates the test of PLAN.md §9 obligation 1.
//!
//! The policy below opts in, so this file contains a generated `#[test]`
//! named `__brocanth_policy_eval_post_policy::eval_agrees_with_the_query`.
//! It runs every rule for every listed subject against a real SQLite
//! database. The hand-written test after it calls the same harness with a
//! policy whose two interpretations disagree, and asserts that the harness
//! finds the disagreement: a harness that cannot fail proves nothing.

mod sqlite;

use brocanth_policy::agreement::policy_agrees;
use brocanth_policy::{Dialect, EntityPred, Opaque, Policy, Pred, Truth};

#[derive(Clone, Copy, Debug, Default)]
pub struct Visitor {
    pub user_id: Option<i64>,
    pub admin: bool,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
        score: i64,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        view: author_id is who.user_id else null && !(status == "draft"),
        update: author_id == who.user_id else deny && score < 3,
        delete: status in ["draft", "archived"] && author_id is not null,

        eval {
            subjects: [
                Visitor::default(),
                Visitor { user_id: Some(1), admin: false },
                Visitor { user_id: Some(2), admin: false },
                Visitor { user_id: Some(0), admin: true },
            ],
            database: sqlite::posts,
            dialect: Sqlite,
            cases: 48,
        }
    }
}

/// A rule whose evaluator lies: it admits every row, and its SQL admits
/// none.
struct Liar;

impl Policy for Liar {
    type Entity = Post;
    type Subject = ();

    fn list(&self, _: &()) -> EntityPred<Post> {
        Pred::Opaque(Opaque::new("a lie", "1 = 0", Vec::new()).with_eval(|_| Truth::True))
    }
}

#[test]
fn the_harness_finds_a_rule_whose_two_interpretations_disagree() {
    let failure = policy_agrees(&Liar, &[()], Dialect::Sqlite, 8, sqlite::posts)
        .expect_err("the evaluator says yes and the query says no");
    assert!(failure.contains("rule `list` for subject 0"), "{failure}");
}

#[test]
fn the_harness_passes_the_generated_policy_on_its_own_too() {
    let subjects = [
        Visitor::default(),
        Visitor {
            user_id: Some(1),
            admin: false,
        },
    ];
    policy_agrees(&PostPolicy, &subjects, Dialect::Sqlite, 16, sqlite::posts)
        .expect("the macro policy agrees with its SQL");
}
