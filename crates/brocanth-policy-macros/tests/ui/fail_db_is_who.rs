//! With `#[db]`, `is` with a subject value is refused: `NULL IS NOT
//! DISTINCT FROM NULL` matches a missing setting to every row with no author.

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
        list: author_id is who.user_id else null,
        view: author_id is not who.user_id else null,
    }
}

fn main() {}
