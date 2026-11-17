#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub score: f64,
    #[attr]
    pub roles: Vec<String>,
}

fn main() {}
