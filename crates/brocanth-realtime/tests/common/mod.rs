//! One entity, one policy, shared by the test binaries.
#![allow(dead_code)]

use brocanth_policy::{Bound, Column, Entity, EntityPred, Ident, Policy, Pred};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn {
    Id,
    AuthorId,
    TeamId,
    Status,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[
        PostColumn::Id,
        PostColumn::AuthorId,
        PostColumn::TeamId,
        PostColumn::Status,
    ];

    fn name(&self) -> &'static str {
        match self {
            PostColumn::Id => "id",
            PostColumn::AuthorId => "author_id",
            PostColumn::TeamId => "team_id",
            PostColumn::Status => "status",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

/// Whoever is watching. Anonymous is `None` everywhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct Visitor {
    pub user_id: Option<i64>,
    pub admin: bool,
}

impl Visitor {
    pub fn user(id: i64) -> Visitor {
        Visitor {
            user_id: Some(id),
            admin: false,
        }
    }

    pub fn admin() -> Visitor {
        Visitor {
            user_id: Some(0),
            admin: true,
        }
    }

    pub fn anonymous() -> Visitor {
        Visitor::default()
    }
}

/// A policy whose list rule inverts and whose view rule inverts differently,
/// so a test can tell which one a topic was gated on.
pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Visitor;

    /// Published posts, or your own.
    fn list(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin {
            return Pred::Always;
        }
        Pred::any([
            Pred::eq(PostColumn::Status, "published"),
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id)),
        ])
    }

    /// Only your own, whatever its status.
    fn view(&self, who: &Visitor) -> EntityPred<Post> {
        if who.admin {
            return Pred::Always;
        }
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.user_id))
    }
}

/// A policy that does not invert: `status <> 'draft'` is a complement.
pub fn not_draft() -> Pred<PostColumn> {
    Pred::negate(Pred::eq(PostColumn::Status, "draft"))
}
