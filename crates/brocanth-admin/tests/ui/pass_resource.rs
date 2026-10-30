//! The same resource as the failing cases, with its policy. It compiles, so
//! the failures are about the policy and nothing else.

mod support;

use brocanth_admin::{AdminConfig, AdminResource};
use support::{Post, PostPolicy, Staff};

pub struct Posts;

impl AdminResource for Posts {
    type Entity = Post;
    type Subject = Staff;
    type Policy = PostPolicy;

    const MODEL: &'static str = "post";

    fn policy(&self) -> &PostPolicy {
        &PostPolicy
    }
}

fn main() {
    let _ = AdminConfig::new().resource(Posts);
}
