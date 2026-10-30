pub struct Visitor;

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        title: String,
    }

    pub policy PostPolicy(who: &Visitor) for Post {
        list: always,
        writable: only [titel],
    }
}

fn main() {}
