//! With `#[db]`, a `{ … }` block is Rust: the database cannot run it. That
//! holds for a block that returns `Pred::Opaque` too, and for a guard.

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
        list: { brocanth_policy::Pred::Always },
        view: {
            brocanth_policy::Pred::Opaque(brocanth_policy::Opaque::new("a function", "f(?)", vec![]))
        },
        update {
            { who.admin } => always,
        }
    }
}

fn main() {}
