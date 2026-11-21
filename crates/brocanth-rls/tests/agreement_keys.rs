//! The agreement harness over the key gate (docs/policy-rls-bridge.md
//! decision 14): a guard arm that reads no column becomes
//! `"key" >= CASE WHEN <guard> THEN <least> END`, with the least value of
//! the key's type. One policy per key type: each must admit the same rows
//! as the application, for each command and each subject.
//!
//! Skipped without `BROCANTH_TEST_DATABASE_URL` (a superuser URL).
#![allow(missing_docs)]

use brocanth_rls::agreement::{check, test_database_url};
use chrono::{DateTime, Utc};
use uuid::Uuid;

#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Who {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub admin: bool,
    #[attr]
    pub banned: bool,
}

fn subjects() -> Vec<Who> {
    let user = |id| Who {
        user_id: Some(id),
        ..Who::default()
    };
    vec![
        Who::default(),
        user(1),
        user(2),
        Who {
            admin: true,
            ..user(3)
        },
        Who {
            banned: true,
            ..user(1)
        },
    ]
}

/// The rules of each policy below: `always` as the fallback (`list`), as
/// the first arm (`update`) and as the second (`delete`), each next to an
/// arm that reads a column.
macro_rules! keyed {
    ($($policy:ident for $entity:ident in $table:literal, key: $key:ty;)*) => {
        brocanth_policy::policy! {
            $(
                pub entity $entity in $table {
                    #[key] id: $key,
                    author_id: Option<i64>,
                    status: String,
                }

                #[db]
                pub policy $policy(who: &Who) for $entity {
                    list {
                        who.banned => author_id == who.user_id else deny,
                        always,
                    }
                    create: author_id == who.user_id else deny,
                    update {
                        who.admin => always,
                        author_id == who.user_id else deny && status == "draft",
                    }
                    delete {
                        who.banned => never,
                        who.admin => always,
                        author_id == who.user_id else deny,
                    }
                }
            )*
        }
    };
}

keyed! {
    SmallKey for Small in "small_keyed", key: i16;
    IntKey for Int in "int_keyed", key: i32;
    BigKey for Big in "big_keyed", key: i64;
    FloatKey for Float in "float_keyed", key: f64;
    TextKey for Text in "text_keyed", key: String;
    BytesKey for Bytes in "bytes_keyed", key: Vec<u8>;
    UuidKey for Uuids in "uuid_keyed", key: Uuid;
    TimeKey for Time in "time_keyed", key: DateTime<Utc>;
}

macro_rules! require_db {
    () => {
        match test_database_url() {
            Some(url) => url,
            None => {
                eprintln!("!! SKIPPED: BROCANTH_TEST_DATABASE_URL is not set");
                return;
            }
        }
    };
}

#[test]
fn the_key_gate_agrees_for_each_key_type() {
    let url = require_db!();
    check::<SmallKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("smallint: {e}"));
    check::<IntKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("integer: {e}"));
    check::<BigKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("bigint: {e}"));
    check::<FloatKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("double: {e}"));
    check::<TextKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("text: {e}"));
    check::<BytesKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("bytea: {e}"));
    check::<UuidKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("uuid: {e}"));
    check::<TimeKey>(&url, &subjects(), 16).unwrap_or_else(|e| panic!("timestamptz: {e}"));
}
