use brocanth_policy::Subject;

#[derive(brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub admin: bool,
    pub display_name: String,
}

#[derive(brocanth_policy::Subject)]
pub struct Generic<T> {
    #[attr]
    pub tenant_id: i32,
    pub extra: T,
}

fn main() {
    let who = Visitor {
        user_id: Some(1),
        admin: false,
        display_name: String::new(),
    };
    assert_eq!(<Visitor as Subject>::ATTRS.len(), 2);
    assert!(who.attributes().get("admin").is_some());
    assert_eq!(<Generic<u8> as Subject>::ATTRS[0].key, "tenant_id");
}
