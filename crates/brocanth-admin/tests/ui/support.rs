//! The parts of an admin resource that are not the policy, so each case
//! shows only the thing it is about.

#![allow(dead_code)]

use brocanth_admin::row_policy::{Column, Entity, EntityPred, Ident, Policy, Pred};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn {
    Id,
    AuthorId,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId];
    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("post");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

/// A second table, so "a policy about the wrong entity" has somewhere to
/// come from.
pub struct Comment;

impl Entity for Comment {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("comment");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

pub struct Staff;

pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Staff;
    fn list(&self, _who: &Staff) -> EntityPred<Post> {
        Pred::Always
    }
}

/// A policy for the other table.
pub struct CommentPolicy;

impl Policy for CommentPolicy {
    type Entity = Comment;
    type Subject = Staff;
    fn list(&self, _who: &Staff) -> EntityPred<Comment> {
        Pred::Always
    }
}
