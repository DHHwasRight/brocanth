//! One entity, one policy, shared by the test binaries.
#![allow(dead_code)]

use brocanth_policy::Pred;

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

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        team_id: Option<i64>,
        status: String,
    }

    /// A policy whose list rule inverts and whose view rule inverts
    /// differently, so a test can tell which one a topic was gated on.
    pub policy PostPolicy(who: &Visitor) for Post {
        /// Published posts, or your own.
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny,
        }
        /// Only your own, whatever its status.
        view {
            who.admin => always,
            author_id == who.user_id else deny,
        }
    }
}

/// A policy that does not invert: `status <> 'draft'` is a complement.
pub fn not_draft() -> Pred<PostColumn> {
    Pred::negate(Pred::eq(PostColumn::Status, "draft"))
}
