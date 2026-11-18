//! `db { … }` compares a policy's rules with the database policy. Without
//! `#[db]`, the policy has neither.

#[derive(Default, brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: author_id == who.user_id else deny,
        db { subjects: [Visitor::default()] }
    }
}

fn main() {}
