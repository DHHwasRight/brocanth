//! With `#[db]`, a rule reads only `#[attr]` fields: `boss_id` never
//! reaches the database, so the database cannot compare with it.

#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub admin: bool,
    pub boss_id: Option<i64>,
    pub owner: bool,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    #[db]
    pub policy PostPolicy(who: &Visitor) for Post {
        list {
            who.owner => always,
            author_id == who.user_id else deny,
        }
        update: author_id == who.boss_id else deny,
    }
}

fn main() {}
