//! `update_check` and `#[app_only]` mean something only with `#[db]`. Without
//! it, an author could think the database checks them.

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

    pub policy PostPolicy(who: &Visitor) for Post {
        list: always,
        #[app_only("reads the clock")]
        update: author_id == who.user_id else deny,
        update_check: status == "draft",
    }
}

fn main() {}
