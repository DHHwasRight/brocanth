//! The entity names its model struct, so the admin's column names and the
//! policy's cannot drift apart.

pub struct PostModel {
    pub id: i32,
    pub author_id: i32,
    pub title: String,
}

brocanth_policy::policy! {
    #[model(PostModel)]
    pub entity Post in "post" {
        #[key] id: i32,
        author_id: i64,
        titel: String,
    }
}

fn main() {}
