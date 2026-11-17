//! With `#[db]`, a guard reads `who.<field>`, `!`, `== literal`, `&&` and
//! `||`. A method call or another comparison has no database form.

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
        list {
            who.user_id.is_none() => never,
            always,
        }
        update {
            who.name.len() > 3 => always,
        }
    }
}

fn main() {}
