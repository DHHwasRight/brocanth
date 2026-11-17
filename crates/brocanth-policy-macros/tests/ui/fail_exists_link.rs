//! `exists` says which row of the link table belongs to this row.

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
        list: exists PostTeam(team_id == who.user_id else deny),
    }
}

fn main() {}
