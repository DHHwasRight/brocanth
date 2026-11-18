//! [`identity()`]: a policy subject as the [`DbIdentity`] a request runs as.
//!
//! docs/policy-rls-bridge.md, decision 6. The identity is built in one
//! place, and every key is always set:
//!
//! - the role is the one role every request under a policy runs as
//!   (decision 7, `app_user`);
//! - each attribute in [`Subject::ATTRS`] becomes the setting
//!   `brocanth.<key>`, typed by its [`AttrKind`];
//! - an absent value (`None`) sets `''`. A policy reads the setting through
//!   `NULLIF(…, '')`, so `''` is `NULL`: "absent" means "anonymous" in the
//!   application and in the database.
//!
//! A key that is not set at all would read the same, but only on a fresh
//! connection: a second `assume` in one transaction sets a dropped key to
//! `''`, and a key nobody set is a typo nobody sees. So no key is left out.
//!
//! ```
//! use brocanth_policy::{Attr, AttrKind, Attributes, Subject};
//! use brocanth_rls::{identity, Role};
//!
//! struct Visitor { user_id: Option<i64>, admin: bool }
//!
//! impl Subject for Visitor {
//!     const ATTRS: &'static [Attr] = &[
//!         Attr { key: "user_id", kind: AttrKind::BigInt, optional: true },
//!         Attr { key: "admin", kind: AttrKind::Boolean, optional: false },
//!     ];
//!     fn attributes(&self) -> Attributes {
//!         Attributes::new()
//!             .with("user_id", self.user_id.map(Into::into))
//!             .with("admin", Some(self.admin.into()))
//!     }
//! }
//!
//! let me = identity(&Role::new("app_user"), &Visitor { user_id: None, admin: false });
//! assert_eq!(me.role_name(), Some("app_user"));
//! assert_eq!(
//!     me.settings(),
//!     [("user_id".to_string(), String::new()), ("admin".to_string(), "false".to_string())]
//! );
//! ```

use brocanth_db::DbIdentity;
use brocanth_policy::{Attr, AttrKind, Attributes, Subject, Value};

use crate::def::Role;
use crate::expr::Setting;
use crate::ident::SqlType;

/// Who a request runs as in the database: `role`, and one setting per
/// attribute of `who`.
///
/// Every key in [`Subject::ATTRS`] is set. An absent value sets `''`, which
/// a policy reads as `NULL`. A present value is set through its typed
/// [`Setting`], so a value the policy cannot cast (a hand-written
/// [`Subject`] that returns text for a `bigint` key) is refused when the
/// identity is assumed, with a `help:` line.
///
/// Run the request's statements in `Db::transact_as(&me, …)` or
/// `Orm::transact_as(&me, …)`. The role and the settings end with the
/// transaction.
pub fn identity<S: Subject>(role: &Role, who: &S) -> DbIdentity {
    of_attributes(role, S::ATTRS, &who.attributes())
}

/// [`identity()`] from the attribute list and the values: each key of
/// `attrs` is set, `''` when `values` lacks it.
pub(crate) fn of_attributes(role: &Role, attrs: &[Attr], values: &Attributes) -> DbIdentity {
    let mut me = DbIdentity::new().role(role.name());
    for attr in attrs {
        me = match values.get(attr.key) {
            Some(value) => me.setting(&setting_of(attr), setting_text(value)),
            None => me.set(attr.key, ""),
        };
    }
    me
}

/// The typed [`Setting`] an attribute is read through:
/// `brocanth.<key>`, cast to the attribute's type.
pub fn setting_of(attr: &Attr) -> Setting {
    Setting::new(attr.key, sql_type(attr.kind))
}

/// The SQL type of an attribute kind.
pub fn sql_type(kind: AttrKind) -> SqlType {
    match kind {
        AttrKind::Boolean => SqlType::Boolean,
        AttrKind::Integer => SqlType::Integer,
        AttrKind::BigInt => SqlType::BigInt,
        AttrKind::Text => SqlType::Text,
        AttrKind::Uuid => SqlType::Uuid,
        AttrKind::Timestamptz => SqlType::Custom(kind.sql_type().to_string()),
    }
}

/// A value as the text `set_config` stores and the policy casts back.
///
/// Not `Value`'s `Display`: that one quotes text, for diagnostics. A uuid
/// is hyphenated, lower case. A timestamp is ISO 8601 in UTC with
/// microseconds (`2026-11-17T09:30:00.000000Z`), which `timestamptz` reads
/// whatever the session's `TimeZone` is.
pub fn setting_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Text(s) => s.clone(),
        Value::Bytes(bytes) => {
            let mut out = String::from("\\x");
            for byte in bytes {
                out.push_str(&format!("{byte:02x}"));
            }
            out
        }
        Value::Uuid(_) | Value::Timestamptz(_) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use brocanth_policy::Attributes;

    use super::*;

    /// Every kind, as a hand-written subject.
    struct Everything {
        id: Option<i64>,
        count: i32,
        admin: bool,
        name: Option<String>,
        team: Option<[u8; 16]>,
        seen: Option<i64>,
    }

    impl Subject for Everything {
        const ATTRS: &'static [Attr] = &[
            Attr {
                key: "user_id",
                kind: AttrKind::BigInt,
                optional: true,
            },
            Attr {
                key: "count",
                kind: AttrKind::Integer,
                optional: false,
            },
            Attr {
                key: "admin",
                kind: AttrKind::Boolean,
                optional: false,
            },
            Attr {
                key: "name",
                kind: AttrKind::Text,
                optional: true,
            },
            Attr {
                key: "team_id",
                kind: AttrKind::Uuid,
                optional: true,
            },
            Attr {
                key: "seen_at",
                kind: AttrKind::Timestamptz,
                optional: true,
            },
        ];

        fn attributes(&self) -> Attributes {
            Attributes::new()
                .with("user_id", self.id.map(Value::Int))
                .with("count", Some(Value::from(self.count)))
                .with("admin", Some(Value::Bool(self.admin)))
                .with("name", self.name.clone().map(Value::Text))
                .with("team_id", self.team.map(Value::Uuid))
                .with("seen_at", self.seen.map(Value::Timestamptz))
        }
    }

    fn settings(me: &DbIdentity) -> Vec<(&str, &str)> {
        me.settings()
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    #[test]
    fn every_key_is_set_and_an_absent_value_is_blank() {
        let anonymous = Everything {
            id: None,
            count: 0,
            admin: false,
            name: None,
            team: None,
            seen: None,
        };
        let me = identity(&Role::new("app_user"), &anonymous);
        assert_eq!(me.role_name(), Some("app_user"));
        assert_eq!(
            settings(&me),
            [
                ("user_id", ""),
                ("count", "0"),
                ("admin", "false"),
                ("name", ""),
                ("team_id", ""),
                ("seen_at", ""),
            ]
        );
        me.statements().expect("a blank value is not cast");
    }

    #[test]
    fn each_kind_is_written_as_postgres_reads_it() {
        let mut team = [0u8; 16];
        team[0] = 0xab;
        team[15] = 0x01;
        let who = Everything {
            id: Some(42),
            count: -3,
            admin: true,
            name: Some("O'Brien".to_string()),
            team: Some(team),
            // 2026-11-23T09:30:00.000123Z
            seen: Some(1_795_426_200_000_123),
        };
        let me = identity(&Role::new("app_user"), &who);
        assert_eq!(
            settings(&me),
            [
                ("user_id", "42"),
                ("count", "-3"),
                ("admin", "true"),
                ("name", "O'Brien"),
                ("team_id", "ab000000-0000-0000-0000-000000000001"),
                ("seen_at", "2026-11-23T09:30:00.000123Z"),
            ]
        );
        me.statements().expect("every value fits its type");
    }

    /// A hand-written subject that returns the wrong kind is refused when
    /// the identity is assumed, not by a cast error in a policy.
    #[test]
    fn a_value_of_the_wrong_kind_is_refused_with_help() {
        struct Liar;
        impl Subject for Liar {
            const ATTRS: &'static [Attr] = &[Attr {
                key: "user_id",
                kind: AttrKind::BigInt,
                optional: false,
            }];
            fn attributes(&self) -> Attributes {
                Attributes::new().with("user_id", Some(Value::Text("seven".into())))
            }
        }
        let e = identity(&Role::new("app_user"), &Liar)
            .statements()
            .unwrap_err()
            .to_string();
        assert!(e.contains("bigint"), "{e}");
        assert!(e.contains("\nhelp: "), "{e}");
    }

    /// A key the subject lists but does not fill is set too: absent.
    #[test]
    fn a_key_missing_from_the_values_is_set_blank() {
        struct Sparse;
        impl Subject for Sparse {
            const ATTRS: &'static [Attr] = &[Attr {
                key: "tenant_id",
                kind: AttrKind::BigInt,
                optional: false,
            }];
            fn attributes(&self) -> Attributes {
                Attributes::new()
            }
        }
        let me = identity(&Role::new("app_user"), &Sparse);
        assert_eq!(settings(&me), [("tenant_id", "")]);
    }

    #[test]
    fn the_setting_types_follow_the_kinds() {
        let kinds = [
            (AttrKind::Boolean, "boolean"),
            (AttrKind::Integer, "integer"),
            (AttrKind::BigInt, "bigint"),
            (AttrKind::Text, "text"),
            (AttrKind::Uuid, "uuid"),
            (AttrKind::Timestamptz, "timestamptz"),
        ];
        for (kind, name) in kinds {
            assert_eq!(sql_type(kind).as_sql(), name);
            assert_eq!(kind.sql_type(), name);
        }
    }

    #[test]
    fn bytes_are_written_as_postgres_hex() {
        assert_eq!(setting_text(&Value::Bytes(vec![0, 0xff])), "\\x00ff");
        assert_eq!(setting_text(&Value::Null), "");
    }
}
