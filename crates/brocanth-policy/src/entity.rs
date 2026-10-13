//! What a policy is written *about*: a table, and an enum of its columns.
//!
//! # The typo is a compile error here, and nowhere else
//!
//! PLAN.md §9's vulnerability class is a misspelled field name. The defence is
//! that this crate never accepts a field name from the author. A comparison
//! takes a [`Column`], a `Column` is an entity's own enum, and
//! `Post::Column::AuthorIdd` is not a variant, so it does not compile. There
//! is no `Pred::eq("author_id", …)` overload to fall back to, deliberately,
//! and PLAN.md §9 is the argument for why this crate ships no string-to-filter
//! parser either.
//!
//! The one place a name does arrive as text is [`crate::FieldMask::check`],
//! because `brocanth-model`'s `Tracked::changed()` returns
//! `Vec<&'static str>`. That path checks every name against [`Column::ALL`]
//! and rejects one it does not recognise, so the mismatch is an error rather
//! than a silent permit.
//!
//! # Implementing it
//!
//! By hand, for now. PLAN.md §12 puts the `policy!` macro at item 10, after a
//! real application has used the algebra.
//!
//! ```
//! use brocanth_policy::{Column, Entity, Ident};
//!
//! #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
//! pub enum PostColumn { Id, AuthorId, TeamId, Status }
//!
//! impl Column for PostColumn {
//!     const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::TeamId, Self::Status];
//!     fn name(&self) -> &'static str {
//!         match self {
//!             Self::Id => "id",
//!             Self::AuthorId => "author_id",
//!             Self::TeamId => "team_id",
//!             Self::Status => "status",
//!         }
//!     }
//! }
//!
//! pub struct Post;
//!
//! impl Entity for Post {
//!     type Column = PostColumn;
//!     const TABLE: Ident = Ident::must("posts");
//!     const PRIMARY_KEY: Self::Column = PostColumn::Id;
//! }
//! ```

use std::fmt;
use std::hash::Hash;

use crate::errors::CompileError;

/// A SQL identifier this crate is willing to write into a statement.
///
/// Values never become SQL text — they are bind parameters, always, and
/// `tests/injection.rs` proves it. Identifiers have to become text, because a
/// column name cannot be a bind parameter in any dialect. So identifiers get
/// the other defence, twice over:
///
/// 1. **They cannot come from user data.** Every constructor takes
///    `&'static str`, which a request body is not. The only way to produce one
///    from a runtime string is to leak it, which is a deliberate act that
///    shows up in review.
/// 2. **They are checked anyway**, against `[A-Za-z_][A-Za-z0-9_]*`, and the
///    compiler also quotes them. A name that would need quoting to be safe is
///    rejected before it reaches the quoting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ident(&'static str);

impl Ident {
    /// Check a name. `Err` names the offending byte.
    pub const fn new(name: &'static str) -> Result<Self, CompileError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() {
            return Err(CompileError::EmptyIdentifier);
        }
        match bytes[0] {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {}
            _ => return Err(CompileError::BadIdentifier { name }),
        }
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' => {}
                _ => return Err(CompileError::BadIdentifier { name }),
            }
            i += 1;
        }
        Ok(Ident(name))
    }

    /// The same check, in a `const` position, where a bad name is a build
    /// failure rather than a runtime `Err`.
    ///
    /// This is what [`Entity::TABLE`] uses. A table name is program text, so
    /// the only honest time to reject it is at compile time.
    ///
    /// # Panics
    ///
    /// At compile time, when `name` is not a bare SQL identifier.
    pub const fn must(name: &'static str) -> Self {
        match Self::new(name) {
            Ok(ident) => ident,
            Err(_) => panic!("not a bare SQL identifier"),
        }
    }

    /// The name, unquoted.
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for Ident {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

/// One entity's columns, as an enum.
///
/// `Copy + Eq + Hash` because [`crate::Audience`] keys a broadcast on a column
/// and [`crate::FieldMask`] holds sets of them.
pub trait Column: Copy + Eq + Hash + fmt::Debug + 'static {
    /// Every column, in declaration order.
    ///
    /// Required rather than derived, because two things need it:
    /// [`crate::FieldMask::check`] rejects a changed-field name that is not
    /// one of these, and a snapshot test can enumerate a table.
    const ALL: &'static [Self];

    /// The column name as the database spells it — the same string
    /// `brocanth-model`'s `Tracked::changed()` produces, because both come
    /// from the entity's own column enum.
    fn name(&self) -> &'static str;

    /// The checked form of [`Column::name`].
    ///
    /// Not a `const`, because a trait cannot compute one per variant. The SQL
    /// compiler calls this and propagates the error, so a column whose name is
    /// not a bare identifier fails the query rather than escaping into it.
    fn ident(&self) -> Result<Ident, CompileError> {
        Ident::new(self.name())
    }
}

/// A table, and the column enum a policy over it is written in.
pub trait Entity: 'static {
    /// The column enum.
    type Column: Column;

    /// The table a list query selects from and a write updates.
    const TABLE: Ident;

    /// The column an admission query matches one row on, and the column
    /// `UPDATE … WHERE id = ? AND <condition>` keys on.
    const PRIMARY_KEY: Self::Column;
}
