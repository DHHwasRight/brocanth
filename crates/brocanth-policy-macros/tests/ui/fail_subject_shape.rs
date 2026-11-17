#[derive(brocanth_policy::Subject)]
pub struct Tuple(#[attr] i64);

#[derive(brocanth_policy::Subject)]
pub enum Who {
    Anonymous,
}

#[derive(brocanth_policy::Subject)]
pub struct Args {
    #[attr(key = "uid")]
    pub user_id: i64,
}

fn main() {}
