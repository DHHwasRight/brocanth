//! `!` over more than one `exists` turns its `else deny` around: an absent
//! value would grant. `!exists` alone keeps it.

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
        list: !(exists PostTeam(post_id == id, team_id == who.user_id else deny) || status == "x"),
    }
}

fn main() {}
