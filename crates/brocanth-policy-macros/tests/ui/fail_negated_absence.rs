//! `else deny` is `never` for an absent value, and `!never` is `always`: the
//! negation would grant access to the visitor the word was written to refuse.

pub struct Visitor {
    pub user_id: Option<i64>,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: !(author_id == who.user_id else deny),
    }
}

fn main() {}
