pub struct Visitor {
    pub id: i64,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: i64,
        status: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: status == "published" || authr_id == who.id,
        update: owner == who.id,
    }
}

fn main() {}
