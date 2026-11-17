//! With `#[db]`, `else allow` is refused: a missing setting would open the
//! table.

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
        list: author_id == who.user_id else allow,
    }
}

fn main() {}
