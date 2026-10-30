pub struct Visitor {
    pub id: i64,
    pub name: String,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: i64,
        status: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: status == who.id,
        update: author_id == who.name,
    }
}

fn main() {}
