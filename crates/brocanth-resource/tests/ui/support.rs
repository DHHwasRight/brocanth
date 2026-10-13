//! The parts of a resource that are not the policy, so each case shows only
//! the thing it is about.

#![allow(dead_code)]

use brocanth_resource::data::{DataError, FetchQuery, ListQuery, ResourceData};
use brocanth_resource::policy::{
    Column, Entity, EntityPred, Ident, Policy, Pred, Value,
};
use brocanth_resource::sql::Statement;

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
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

/// A second table, so "a policy about the wrong entity" has somewhere to come
/// from.
pub struct Comment;

impl Entity for Comment {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("comments");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

pub struct Visitor;

pub struct PostPolicy;

impl Policy for PostPolicy {
    type Entity = Post;
    type Subject = Visitor;
    fn list(&self, _who: &Visitor) -> EntityPred<Post> {
        Pred::Always
    }
}

/// A policy for the other table.
pub struct CommentPolicy;

impl Policy for CommentPolicy {
    type Entity = Comment;
    type Subject = Visitor;
    fn list(&self, _who: &Visitor) -> EntityPred<Comment> {
        Pred::Always
    }
}

/// Not a policy at all.
pub struct Whatever;

pub struct Row;

pub struct Rows;

impl ResourceData<Post> for Rows {
    type Row = Row;

    fn count(&self, _query: &ListQuery<'_, PostColumn>) -> Result<u64, DataError> {
        Ok(0)
    }

    fn list(&self, _query: &ListQuery<'_, PostColumn>) -> Result<Vec<Row>, DataError> {
        Ok(Vec::new())
    }

    fn fetch(&self, _query: &FetchQuery<'_, PostColumn>) -> Result<Option<Row>, DataError> {
        Ok(None)
    }

    fn admit(&self, _statement: &Statement) -> Result<usize, DataError> {
        Ok(0)
    }

    fn write(&self, _statement: &Statement) -> Result<u64, DataError> {
        Ok(0)
    }

    fn insert(&self, _values: &[(PostColumn, Value)]) -> Result<(Value, Row), DataError> {
        Err(DataError::backend("no"))
    }
}
