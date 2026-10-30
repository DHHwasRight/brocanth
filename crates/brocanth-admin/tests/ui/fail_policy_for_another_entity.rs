//! A policy borrowed from another table does not compile. Its conditions
//! would name the other table's columns, and the guarded select would run
//! them against this one.

mod support;

use brocanth_admin::{AdminConfig, AdminResource};
use support::{CommentPolicy, Post, Staff};

pub struct Posts;

impl AdminResource for Posts {
    type Entity = Post;
    type Subject = Staff;
    type Policy = CommentPolicy;

    const MODEL: &'static str = "post";

    fn policy(&self) -> &CommentPolicy {
        &CommentPolicy
    }
}

fn main() {
    let _ = AdminConfig::new().resource(Posts);
}
