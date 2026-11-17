//! With `#[db]`, a value from the subject is exactly `who.<field>`: the
//! database reads the field from a setting and cannot run the rest.

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
        list: author_id == who.user_id.map(|id| id + 1) else deny,
        view: status == who.name.to_lowercase(),
    }
}

fn main() {}
