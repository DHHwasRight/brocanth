//! With `#[db]`, a value that is not from the subject is a literal or a
//! constant: the database policy holds it as written at `rls make`.

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
        list: status == String::from("published"),
        view: status in vec!["a", "b"],
    }
}

fn main() {}
