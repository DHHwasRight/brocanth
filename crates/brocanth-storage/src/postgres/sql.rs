//! Every statement the Postgres backend runs, written out in full.
//!
//! One dialect, so there is no `Dialect` match here and no template: the
//! statements are readable SQL that pastes into `psql` when something is
//! wrong, which is the same property `brocanth-jobs`'s `store/sql.rs` is
//! after.
//!
//! # The DDL, and what each decision buys
//!
//! ```sql
//! CREATE TABLE brocanth_objects (
//!     key          BYTEA  PRIMARY KEY CHECK (octet_length(key) = 32),
//!     content      BYTEA  NOT NULL,
//!     size         BIGINT NOT NULL,
//!     content_type TEXT   NOT NULL,
//!     filename     TEXT
//! );
//!
//! CREATE TABLE brocanth_object_variants (
//!     key     BYTEA NOT NULL REFERENCES brocanth_objects (key) ON DELETE CASCADE,
//!     spec    TEXT  NOT NULL,
//!     content BYTEA NOT NULL,
//!     PRIMARY KEY (key, spec)
//! );
//! ```
//!
//! * **`key` is `BYTEA`, not hex `TEXT`.** An
//!   [`ObjectKey`](crate::key::ObjectKey) is thirty-two bytes; the hex form is
//!   sixty-four characters plus a varlena header, so the text column is more
//!   than twice the index. Worse, text admits two spellings of one address
//!   unless a `CHECK` forbids the uppercase one, and two spellings of one
//!   address are two rows that should have been one — the same argument
//!   [`ObjectKey::parse`](crate::key::ObjectKey::parse) makes for refusing
//!   uppercase. `bytea` admits exactly one spelling, and neither side formats
//!   anything.
//! * **The `CHECK` is documentation the database enforces.** Nothing in this
//!   crate can produce a key of another width, but the table outlives this
//!   crate and a hand-written `INSERT` is one `psql` session away.
//! * **`size` is a column rather than `octet_length(content)`.**
//!   [`Storage::meta`](crate::storage::Storage::meta) exists to answer a
//!   `HEAD` *without reading the content*, and `octet_length` on a TOASTed
//!   value fetches and decompresses it to count the bytes. A `BIGINT` beside
//!   the pointer means `meta` and `exists` never touch the TOAST table at all.
//! * **Two tables rather than one with a nullable `spec`.** A nullable column
//!   cannot sit in a primary key, so the single-table design needs a sentinel
//!   spec for "the object itself", and then the parent-child link is a
//!   self-reference that no foreign key can express. `size`, `content_type`
//!   and `filename` are also meaningless for a variant, so one table means
//!   three columns that are `NULL` on every variant row and a `CHECK` to keep
//!   them that way. Two tables and one foreign key say the same thing, and the
//!   database enforces it.
//! * **`ON DELETE CASCADE`, and it is the point of the foreign key.** A
//!   thumbnail of a deleted document is still the document, so a variant must
//!   not outlive its parent. `LocalDisk` gets this by removing the variant
//!   directory first and hoping the process survives to the second `unlink`;
//!   the cascade is one statement, in one transaction, with no window in
//!   between. It also makes an orphaned variant unrepresentable rather than
//!   merely unlikely.
//! * **The foreign key is indexed, because Postgres does not do that for
//!   you.** A referencing column with no index turns every parent delete into
//!   a sequential scan of the child table. Here `PRIMARY KEY (key, spec)`
//!   leads with `key`, so the cascade's lookup uses it and no second index is
//!   needed.
//!
//! # TOAST, and the one knob worth turning
//!
//! `content` is left at `bytea`'s default storage, `EXTENDED`: Postgres tries
//! to compress the value and moves it out of line into the TOAST table when it
//! does not fit in a page. A deployment that stores only JPEG, PNG and WebP —
//! all already compressed — can skip the attempt with
//!
//! ```sql
//! ALTER TABLE brocanth_objects ALTER COLUMN content SET STORAGE EXTERNAL;
//! ```
//!
//! which is not the default here because the compression attempt is cheap, it
//! gives up early on incompressible input, and a store holding PDFs, CSVs and
//! SVGs wants it.

/// The schema, applied in order by
/// [`Postgres::migrate`](super::Postgres::migrate).
///
/// Public because an application with its own migration runner — `sqlx
/// migrate`, `refinery`, a `.sql` file somebody reviews — wants the DDL rather
/// than a method that applies it behind its back. The statements below it are
/// not public: they are this backend's business, and a caller running them by
/// hand is a caller writing a second implementation of `put`.
///
/// Every statement is `IF NOT EXISTS`, so applying it to a database that
/// already has the tables is a no-op. That is **not** on its own enough to
/// make two simultaneous first migrations safe — `IF NOT EXISTS` checks the
/// catalogue and then creates, and the loser of that race fails on a duplicate
/// key in `pg_type` — which is why [`Postgres::migrate`](super::Postgres::migrate)
/// holds an advisory lock around the whole list.
pub const SCHEMA: &[&str] = &[
    r#"CREATE TABLE IF NOT EXISTS brocanth_objects (
        key          BYTEA  PRIMARY KEY CHECK (octet_length(key) = 32),
        content      BYTEA  NOT NULL,
        size         BIGINT NOT NULL,
        content_type TEXT   NOT NULL,
        filename     TEXT
    )"#,
    // The foreign key is what makes an orphaned variant unrepresentable, and
    // the composite primary key is what makes the cascade's lookup an index
    // scan. See the module documentation.
    r#"CREATE TABLE IF NOT EXISTS brocanth_object_variants (
        key     BYTEA NOT NULL REFERENCES brocanth_objects (key) ON DELETE CASCADE,
        spec    TEXT  NOT NULL,
        content BYTEA NOT NULL,
        PRIMARY KEY (key, spec)
    )"#,
];

/// `$1` key, `$2` content, `$3` size, `$4` content type, `$5` filename.
///
/// `DO NOTHING` and not `DO UPDATE`, and that is the security property rather
/// than a performance one. The second upload of a set of bytes leaves the
/// first uploader's `content_type` and `filename` exactly as they were, so
/// uploading a copy of somebody else's file is not a way to change what their
/// file is served as. See
/// [`Storage::put`](crate::storage::Storage::put).
///
/// It is also the whole of the concurrency story. Two transactions inserting
/// one key do not race: the second takes a speculative insertion lock, waits
/// for the first to commit, finds the conflict and does nothing. There is no
/// check-then-insert window because there is no check.
pub(crate) const INSERT_OBJECT: &str = r#"
INSERT INTO brocanth_objects (key, content, size, content_type, filename)
VALUES ($1, $2, $3, $4, $5)
ON CONFLICT (key) DO NOTHING"#;

pub(crate) const SELECT_CONTENT: &str = "SELECT content FROM brocanth_objects WHERE key = $1";

/// Deliberately not `SELECT *`: reading `content` here would detoast the
/// object to answer a `HEAD`.
pub(crate) const SELECT_META: &str =
    "SELECT size, content_type, filename FROM brocanth_objects WHERE key = $1";

/// `SELECT 1`, so the planner answers from the primary key index alone.
pub(crate) const SELECT_EXISTS: &str = "SELECT 1 FROM brocanth_objects WHERE key = $1";

/// One statement, because the cascade removes the variants. Deleting a key
/// that is not there affects no rows and is not an error, which is the
/// idempotence [`Storage::delete`](crate::storage::Storage::delete) promises.
pub(crate) const DELETE_OBJECT: &str = "DELETE FROM brocanth_objects WHERE key = $1";

pub(crate) const SELECT_VARIANT: &str =
    "SELECT content FROM brocanth_object_variants WHERE key = $1 AND spec = $2";

/// `DO NOTHING` again, and here it is simply correct: a variant is a pure
/// function of the object's bytes and the specification, so the second writer
/// is writing the bytes that are already there. Two requests for an uncached
/// thumbnail arriving together is the normal case, and neither of them needs
/// to rewrite the other's row.
pub(crate) const INSERT_VARIANT: &str = r#"
INSERT INTO brocanth_object_variants (key, spec, content)
VALUES ($1, $2, $3)
ON CONFLICT (key, spec) DO NOTHING"#;

#[cfg(test)]
mod tests {
    //! These need no server. They are here so that the properties the module
    //! documentation argues for are checked on every `cargo test`, including
    //! the runs where no database was named and the behavioural tests are
    //! skipped.

    #[test]
    fn the_address_column_is_bytea_and_checked() {
        assert!(super::SCHEMA[0].contains("key          BYTEA  PRIMARY KEY"));
        assert!(super::SCHEMA[0].contains("octet_length(key) = 32"));
    }

    #[test]
    fn the_size_is_a_column_rather_than_a_detoast() {
        assert!(super::SCHEMA[0].contains("size         BIGINT NOT NULL"));
        assert!(!super::SELECT_META.contains("octet_length"));
        // `content_type` is fine; the `content` column is the one that would
        // detoast the object to answer a `HEAD`.
        assert!(!super::SELECT_META.contains("SELECT content"));
        assert!(!super::SELECT_META.contains(", content,"));
        assert!(!super::SELECT_META.contains(" content "));
    }

    #[test]
    fn a_variant_cannot_outlive_its_parent() {
        assert!(super::SCHEMA[1].contains("REFERENCES brocanth_objects (key) ON DELETE CASCADE"));
        // And the cascade's lookup is indexed, because Postgres does not index
        // a referencing column on its own.
        assert!(super::SCHEMA[1].contains("PRIMARY KEY (key, spec)"));
    }

    #[test]
    fn no_insert_ever_rewrites_a_row() {
        // The metadata-rewrite attack is refused by the statement text, so the
        // check is worth making against the statement text.
        for statement in [super::INSERT_OBJECT, super::INSERT_VARIANT] {
            assert!(statement.contains("ON CONFLICT"));
            assert!(statement.contains("DO NOTHING"));
            assert!(!statement.contains("DO UPDATE"));
            assert!(!statement.contains("excluded"));
        }
    }

    #[test]
    fn the_schema_is_safe_to_apply_twice() {
        for statement in super::SCHEMA {
            assert!(statement.contains("IF NOT EXISTS"));
        }
    }
}
