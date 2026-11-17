//! The link entity is declared in the same block, and each name in `exists`
//! is a column: a typo is a compile error on the typo.

#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub admin: bool,
    #[attr]
    pub name: String,
    pub team_ids: Vec<i64>,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        status: String,
    }

    pub entity PostTeam in "post_teams" {
        #[key] post_id: i64,
        team_id: i64,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: exists Team(post_id == id),
        view: exists PostTeam(post_id == idd),
        update: exists PostTeam(post_idd == id),
    }
}

fn main() {}
