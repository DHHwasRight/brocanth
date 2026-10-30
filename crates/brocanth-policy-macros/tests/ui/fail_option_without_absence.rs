//! PLAN.md §9: `@request.auth.rolee != "banned"` grants access in
//! PocketBase. Here an absent value must be given a meaning.

pub struct Visitor {
    pub user_id: Option<i64>,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: author_id != who.user_id,
    }
}

fn main() {}
