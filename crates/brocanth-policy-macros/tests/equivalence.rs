//! The macro emits what a person writes by hand, and nothing else.
//!
//! Each case declares one policy twice: by hand, as the call sites in
//! `brocanth-resource`, `brocanth-realtime` and `brocanth-admin-model` write
//! it today, and with `policy!`. Then it asserts, for every subject and every
//! rule, that the two give the same `Pred` value and the same SQL text and
//! binds in both dialects, and the same field mask.

use brocanth_policy::{Column, Dialect, Entity, EntityPred, FieldMask, Policy, Pred};

/// Whoever is asking. The resource tests' subject, unchanged.
#[derive(Clone, Copy, Debug, Default)]
pub struct Visitor {
    pub user_id: Option<i64>,
    pub admin: bool,
    pub banned: bool,
}

const VISITORS: [Visitor; 5] = [
    Visitor {
        user_id: None,
        admin: false,
        banned: false,
    },
    Visitor {
        user_id: Some(1),
        admin: false,
        banned: false,
    },
    Visitor {
        user_id: Some(2),
        admin: false,
        banned: false,
    },
    Visitor {
        user_id: Some(0),
        admin: true,
        banned: false,
    },
    Visitor {
        user_id: Some(7),
        admin: false,
        banned: true,
    },
];

mod by_macro {
    use super::Visitor;

    brocanth_policy::policy! {
        pub entity Post in "posts" {
            #[key] id: i64,
            author_id: Option<i64>,
            status: String,
            title: String,
        }

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
            writable {
                who.admin => except [id],
                only [title, author_id],
            }
        }

        /// Every other shape of the grammar, so each has a hand-written twin.
        pub policy Shapes(who: &Visitor) for Post {
            list: !(status == "draft") && (author_id is not null || title in ["a", "b"]),
            view: status not in vec!["x".to_owned(), "y".to_owned()] && author_id is null,
            create: author_id is who.user_id else null || author_id is not who.user_id else allow,
            update: author_id < 10 && author_id <= 9 && author_id > 0 && author_id >= 1 && author_id != 5,
            delete: { brocanth_policy::Pred::negate(brocanth_policy::Pred::eq(PostColumn::Title, "x")) },
            writable: { brocanth_policy::FieldMask::only([PostColumn::Title]) },
        }
    }
}

mod by_hand {
    use brocanth_policy::{Bound, Column, Entity, EntityPred, FieldMask, Ident, Policy, Pred};

    use super::Visitor;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub enum PostColumn {
        Id,
        AuthorId,
        Status,
        Title,
    }

    impl Column for PostColumn {
        const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status, Self::Title];

        fn name(&self) -> &'static str {
            match self {
                Self::Id => "id",
                Self::AuthorId => "author_id",
                Self::Status => "status",
                Self::Title => "title",
            }
        }
    }

    pub struct Post;

    impl Entity for Post {
        type Column = PostColumn;
        const TABLE: Ident = Ident::must("posts");
        const PRIMARY_KEY: Self::Column = PostColumn::Id;
    }

    /// `brocanth-resource/tests/common/mod.rs`, as it was written.
    pub struct PostPolicy;

    impl Policy for PostPolicy {
        type Entity = Post;
        type Subject = Visitor;

        fn list(&self, who: &Visitor) -> EntityPred<Post> {
            if who.banned {
                return Pred::Never;
            }
            if who.admin {
                return Pred::Always;
            }
            Pred::any([
                Pred::eq(PostColumn::Status, "published"),
                Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
            ])
        }

        fn create(&self, who: &Visitor) -> EntityPred<Post> {
            match who.user_id {
                Some(id) => Pred::eq(PostColumn::AuthorId, id),
                None => Pred::Never,
            }
        }

        fn update(&self, who: &Visitor) -> EntityPred<Post> {
            if who.admin {
                return Pred::Always;
            }
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
        }

        fn delete(&self, who: &Visitor) -> EntityPred<Post> {
            if who.admin {
                Pred::Always
            } else {
                Pred::Never
            }
        }

        fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {
            if who.admin {
                FieldMask::except([PostColumn::Id])
            } else {
                FieldMask::only([PostColumn::Title, PostColumn::AuthorId])
            }
        }
    }

    pub struct Shapes;

    impl Policy for Shapes {
        type Entity = Post;
        type Subject = Visitor;

        fn list(&self, _: &Visitor) -> EntityPred<Post> {
            Pred::all([
                Pred::negate(Pred::eq(PostColumn::Status, "draft")),
                Pred::any([
                    Pred::is_not_null(PostColumn::AuthorId),
                    Pred::one_of(PostColumn::Title, ["a", "b"]),
                ]),
            ])
        }

        fn view(&self, _: &Visitor) -> EntityPred<Post> {
            Pred::all([
                Pred::none_of(PostColumn::Status, ["x", "y"]),
                Pred::is_null(PostColumn::AuthorId),
            ])
        }

        fn create(&self, who: &Visitor) -> EntityPred<Post> {
            Pred::any([
                Pred::is(PostColumn::AuthorId, Bound::or_null(who.user_id)),
                Pred::is_not(PostColumn::AuthorId, Bound::or_allow(who.user_id)),
            ])
        }

        fn update(&self, _: &Visitor) -> EntityPred<Post> {
            Pred::all([
                Pred::lt(PostColumn::AuthorId, 10),
                Pred::le(PostColumn::AuthorId, 9),
                Pred::gt(PostColumn::AuthorId, 0),
                Pred::ge(PostColumn::AuthorId, 1),
                Pred::ne(PostColumn::AuthorId, 5),
            ])
        }

        fn delete(&self, _: &Visitor) -> EntityPred<Post> {
            Pred::negate(Pred::eq(PostColumn::Title, "x"))
        }

        fn writable(&self, _: &Visitor) -> FieldMask<PostColumn> {
            FieldMask::only([PostColumn::Title])
        }
    }
}

/// Every rule of a policy, by name.
fn rules<P: Policy>(policy: &P, who: &P::Subject) -> Vec<(&'static str, EntityPred<P::Entity>)> {
    vec![
        ("list", policy.list(who)),
        ("view", policy.view(who)),
        ("create", policy.create(who)),
        ("update", policy.update(who)),
        ("delete", policy.delete(who)),
    ]
}

/// The two policies agree on the entity, on every rule and on the mask.
///
/// The column enums are two types, so a `Pred` from one is compared with a
/// `Pred` from the other through its `Debug` text, and the SQL through its
/// text and binds, which is what reaches the database.
fn assert_same<M, H>(by_macro: &M, by_hand: &H)
where
    M: Policy<Subject = Visitor>,
    H: Policy<Subject = Visitor>,
{
    type Cols<P> = <<P as Policy>::Entity as Entity>::Column;

    assert_eq!(
        <M::Entity as Entity>::TABLE,
        <H::Entity as Entity>::TABLE,
        "table"
    );
    let names = |all: &[&'static str]| all.to_vec();
    assert_eq!(
        names(&Cols::<M>::ALL.iter().map(|c| c.name()).collect::<Vec<_>>()),
        names(&Cols::<H>::ALL.iter().map(|c| c.name()).collect::<Vec<_>>()),
        "column names"
    );
    assert_eq!(
        <M::Entity as Entity>::PRIMARY_KEY.name(),
        <H::Entity as Entity>::PRIMARY_KEY.name(),
        "key"
    );

    for (index, who) in VISITORS.iter().enumerate() {
        let made = rules(by_macro, who);
        let hand = rules(by_hand, who);
        for ((action, m), (_, h)) in made.iter().zip(&hand) {
            assert_eq!(
                format!("{m:?}"),
                format!("{h:?}"),
                "rule `{action}`, visitor {index}"
            );
            for dialect in [Dialect::Postgres, Dialect::Sqlite] {
                let (ms, mb) = sql::<M::Entity>(m, dialect);
                let (hs, hb) = sql::<H::Entity>(h, dialect);
                assert_eq!(ms, hs, "SQL of `{action}`, visitor {index}, {dialect:?}");
                assert_eq!(mb, hb, "binds of `{action}`, visitor {index}, {dialect:?}");
            }
        }
        assert_eq!(
            mask(&by_macro.writable(who)),
            mask(&by_hand.writable(who)),
            "writable, visitor {index}"
        );
    }
}

fn sql<E: Entity>(
    pred: &Pred<E::Column>,
    dialect: Dialect,
) -> (String, Vec<brocanth_policy::Value>) {
    let condition = pred
        .condition::<E>(dialect)
        .expect("the predicate compiles");
    let (sql, binds) = condition.into_parts();
    (sql, binds)
}

/// A mask as the set of column names it permits.
fn mask<C: Column>(mask: &FieldMask<C>) -> Vec<&'static str> {
    C::ALL
        .iter()
        .filter(|c| mask.permits(**c))
        .map(|c| c.name())
        .collect()
}

#[test]
fn the_resource_policy_expands_to_the_hand_written_one() {
    assert_same(&by_macro::PostPolicy, &by_hand::PostPolicy);
}

#[test]
fn every_shape_of_the_grammar_expands_to_its_hand_written_twin() {
    assert_same(&by_macro::Shapes, &by_hand::Shapes);
}

#[test]
fn the_macro_entity_is_the_hand_written_entity() {
    use by_macro::PostColumn;
    assert_eq!(
        PostColumn::ALL,
        &[
            PostColumn::Id,
            PostColumn::AuthorId,
            PostColumn::Status,
            PostColumn::Title
        ]
    );
    assert_eq!(by_macro::Post::TABLE.as_str(), "posts");
    assert_eq!(by_macro::Post::PRIMARY_KEY, PostColumn::Id);
}

#[test]
fn an_anonymous_visitor_owns_nothing_and_the_rule_says_so() {
    // `else deny` is `Bound::or_deny`: the comparison folds to `Never`, and
    // the `||` folds it away. The audience inversion of PLAN.md §10 reads the
    // same shape it reads from a hand-written policy.
    let anonymous = Visitor::default();
    let list = by_macro::PostPolicy.list(&anonymous);
    assert_eq!(
        list,
        Pred::eq(by_macro::PostColumn::Status, "published"),
        "only the published rows"
    );
    assert!(by_macro::PostPolicy.create(&anonymous).is_never());
    assert!(list.audience().is_ok(), "the rule still inverts");
}
