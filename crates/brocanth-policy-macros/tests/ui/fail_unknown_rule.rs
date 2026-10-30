pub struct Visitor;

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: always,
        read: always,
    }
}

fn main() {}
