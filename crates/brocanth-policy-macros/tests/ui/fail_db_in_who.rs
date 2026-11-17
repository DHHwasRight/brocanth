//! With `#[db]`, `in` over a list on the subject is refused: a setting holds
//! one value.

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

    #[db]
    pub policy PostPolicy(who: &Visitor) for Post {
        list: author_id in &who.team_ids,
    }
}

fn main() {}
