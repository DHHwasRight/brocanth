//! Every part of the `#[db]` syntax, once.

use brocanth_policy::{DbPolicy, Policy, Pred, Subject};

#[derive(Default, brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub tenant_id: i64,
    #[attr]
    pub admin: bool,
    #[attr]
    pub banned: bool,
    #[attr]
    pub role: String,
    /// Not an attribute: masks and app-only rules may still read it.
    pub team_ids: Vec<i64>,
}

const LOCKED: &str = "locked";

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        tenant_id: i64,
        status: String,
        score: i32,
    }

    pub entity Member in "members" {
        #[key] post_id: i64,
        user_id: i64,
        role: Option<String>,
    }

    /// Doc comments stay on the struct.
    #[db]
    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin || (who.role == "editor" && !who.banned) => always,
            who.role != "guest" => tenant_id == who.tenant_id && score > -1,
            status == "published"
                || author_id == who.user_id else deny
                || exists Member(post_id == id, user_id == who.user_id else deny, role is not null),
        }
        view: !exists Member(post_id == id, user_id == who.user_id else null) && status != LOCKED,
        create: author_id == who.user_id else deny && status in ["draft"],
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        update_check {
            who.admin => always,
            author_id == who.user_id else deny && status not in ["archived"],
        }
        /// Doc comments stay on the method.
        #[app_only("a team list is not a setting")]
        delete: { if who.team_ids.is_empty() { Pred::Never } else { Pred::Always } },
        writable {
            who.admin => except [id],
            { brocanth_policy::FieldMask::only([PostColumn::Status]) },
        }
        // A test only: `#[cfg(test)]`, so `brocanth_rls` need not be here.
        db {
            subjects: [Visitor::default()],
            cases: 8,
            url: std::env::var("APP_DATABASE_URL").ok(),
        }
    }

    #[db]
    pub policy Public(_: &Visitor) for Post {
        list: status == "published",
        db { subjects: [Visitor::default()] }
    }
}

fn main() {
    let who = Visitor::default();
    let _ = PostPolicy.list(&who);
    let rules = PostPolicy::rules();
    assert!(rules.update_check.is_some());
    assert_eq!(rules.delete.app_only, Some("a team list is not a setting"));
    assert!(PostPolicy.delete(&who).is_never());
    assert!(matches!(Public.list(&who), Pred::Cmp { .. }));
    assert_eq!(Visitor::ATTRS.len(), 5);
}
