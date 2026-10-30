brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        created_at: std::time::SystemTime,
    }
}

fn main() {}
