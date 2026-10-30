//! PLAN.md §12, for the admin: an admin resource with no policy registered
//! must not compile.
//!
//! Everything else about this resource is in order. There is no policy, and
//! that alone is the error.

mod support;

use brocanth_admin::{AdminConfig, AdminResource};
use support::{Post, Staff};

pub struct Posts;

impl AdminResource for Posts {
    type Entity = Post;
    type Subject = Staff;

    const MODEL: &'static str = "post";
}

fn main() {
    let _ = AdminConfig::new().resource(Posts);
}
