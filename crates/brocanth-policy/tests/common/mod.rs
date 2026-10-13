//! One entity, shared by every test: a blog post with an author, a team and a
//! status, plus a link table so the `EXISTS` shape has somewhere to point.

#![allow(dead_code)]

use brocanth_policy::{Column, Entity, Ident};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn {
    Id,
    AuthorId,
    TeamId,
    Status,
    Score,
}

impl Column for PostColumn {
    const ALL: &'static [Self] = &[
        Self::Id,
        Self::AuthorId,
        Self::TeamId,
        Self::Status,
        Self::Score,
    ];

    fn name(&self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::TeamId => "team_id",
            Self::Status => "status",
            Self::Score => "score",
        }
    }
}

pub struct Post;

impl Entity for Post {
    type Column = PostColumn;
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}

/// The link table the `EXISTS` shape reaches into.
pub const MEMBERS: Ident = Ident::must("team_members");
/// Its column that matches `posts.team_id`.
pub const MEMBERS_TEAM: Ident = Ident::must("team_id");
/// Its column that identifies the subject.
pub const MEMBERS_USER: Ident = Ident::must("user_id");

/// The schema the agreement tests create, so the SQL this crate generates runs
/// against a table with the columns it names.
pub const SCHEMA: &str = "
    CREATE TABLE posts (
        id        INTEGER PRIMARY KEY,
        author_id INTEGER,
        team_id   INTEGER,
        status    TEXT,
        score     INTEGER
    );
    CREATE TABLE team_members (
        team_id INTEGER NOT NULL,
        user_id INTEGER NOT NULL
    );
";
