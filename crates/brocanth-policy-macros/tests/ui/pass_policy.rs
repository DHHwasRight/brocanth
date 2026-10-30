//! Every part of the syntax, once.

pub struct Visitor {
    pub user_id: Option<i64>,
    pub team_ids: Vec<i64>,
    pub admin: bool,
}

/// The model struct the entity is checked against: a SeaORM `Model` in an
/// application, any struct here.
pub struct PostModel {
    pub id: i64,
    pub author_id: Option<i64>,
    pub team_id: i32,
    pub status: String,
    pub r#type: String,
}

brocanth_policy::policy! {
    /// Blog posts.
    #[model(PostModel)]
    pub entity Post in "posts" {
        #[key] id: i64,
        /// Who wrote it. Anonymous posts have none.
        author_id: Option<i64>,
        team_id: i32,
        status: String,
        r#type: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.admin => always,
            status == "published" || author_id == who.user_id else deny || team_id in &who.team_ids,
        }
        view: author_id is not null && r#type != "secret",
        create: author_id == who.user_id else deny,
        update {
            who.admin => always,
            who.user_id.is_none() => never,
            (author_id == who.user_id else deny) && !(status == "locked"),
        }
        delete {
            who.admin => always,
        }
        writable {
            who.admin => except [id],
            only [status, r#type],
        }
    }

    pub policy Everything(_: &()) for Post {
        list: always,
    }
}

fn main() {
    use brocanth_policy::{Column, Policy};
    let who = Visitor {
        user_id: Some(1),
        team_ids: vec![3],
        admin: false,
    };
    assert!(!PostPolicy.list(&who).is_always());
    assert_eq!(PostColumn::Type.name(), "type");
    assert!(Everything.list(&()).is_always());
}
