//! [`Principal`]: who a transaction runs as, for Postgres row-level
//! security.
//!
//! A policy reads the principal from the transaction:
//! `current_setting('brocanth.<key>', true)` for a value, `current_user` for
//! the role. [`Tx::assume`](crate::Tx::assume) sets both, and both end at
//! `COMMIT` or `ROLLBACK`:
//!
//! ```sql
//! SET LOCAL ROLE "app_member";
//! SELECT set_config($1, $2, true);   -- 'brocanth.tenant_id', '7'
//! ```
//!
//! So the pool hands the connection to the next request with the login role
//! and no value. `docs/rls.md` §4 has the reasons.

use std::fmt;

use crate::error::DbError;

/// The prefix of every setting a [`Principal`] writes. A policy reads
/// `current_setting('brocanth.tenant_id', true)`.
pub const SETTING_PREFIX: &str = "brocanth.";

/// The longest identifier Postgres keeps. A longer one is truncated, so
/// `SET ROLE` would name another role.
pub const MAX_IDENT: usize = 63;

/// Who a transaction runs as: a role, and named values.
///
/// ```
/// use brocanth_db::Principal;
///
/// let me = Principal::new().role("app_member").set("tenant_id", 7).set("user_id", 42);
/// assert_eq!(me.settings().len(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Principal {
    role: Option<String>,
    settings: Vec<(String, String)>,
}

impl Principal {
    /// No role and no value: the transaction keeps the login role.
    pub fn new() -> Principal {
        Principal::default()
    }

    /// `SET LOCAL ROLE "<name>"`. The login role must be a member of it.
    pub fn role(mut self, name: impl Into<String>) -> Principal {
        self.role = Some(name.into());
        self
    }

    /// `set_config('brocanth.<key>', '<value>', true)`. A second value for a
    /// key replaces the first.
    pub fn set(mut self, key: impl Into<String>, value: impl fmt::Display) -> Principal {
        let key = key.into();
        let value = value.to_string();
        match self.settings.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.settings.push((key, value)),
        }
        self
    }

    /// The role, when one is set.
    pub fn role_name(&self) -> Option<&str> {
        self.role.as_deref()
    }

    /// The keys, without the prefix, and their values, in the order they
    /// were set.
    pub fn settings(&self) -> &[(String, String)] {
        &self.settings
    }

    /// The statements [`Tx::assume`](crate::Tx::assume) runs: an optional
    /// `SET LOCAL ROLE`, and one `SELECT set_config(…)` with its binds
    /// (`brocanth.<key>`, value, in pairs).
    ///
    /// # Errors
    /// [`DbError::Config`] for a role name or a key this crate refuses.
    pub(crate) fn statements(&self) -> Result<Statements, DbError> {
        let role = match &self.role {
            Some(name) => Some(format!("SET LOCAL ROLE {}", quote_ident(name)?)),
            None => None,
        };
        if self.settings.is_empty() {
            return Ok(Statements {
                role,
                settings: None,
            });
        }
        let mut sql = String::from("SELECT ");
        let mut binds = Vec::with_capacity(self.settings.len() * 2);
        for (index, (key, value)) in self.settings.iter().enumerate() {
            check_key(key)?;
            if index > 0 {
                sql.push_str(", ");
            }
            let n = index * 2 + 1;
            sql.push_str(&format!("set_config(${n}, ${}, true)", n + 1));
            binds.push(format!("{SETTING_PREFIX}{key}"));
            binds.push(value.clone());
        }
        Ok(Statements {
            role,
            settings: Some((sql, binds)),
        })
    }
}

/// What [`Tx::assume`](crate::Tx::assume) runs for a [`Principal`].
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Statements {
    /// `SET LOCAL ROLE "<name>"`, when the principal has a role.
    pub(crate) role: Option<String>,
    /// `SELECT set_config($1, $2, true), …` and its binds, when it has
    /// values.
    pub(crate) settings: Option<(String, Vec<String>)>,
}

/// `"name"`, with each `"` doubled.
///
/// # Errors
/// [`DbError::Config`] for an empty name, a name over 63 bytes, or a name
/// with a NUL byte.
pub fn quote_ident(name: &str) -> Result<String, DbError> {
    if name.is_empty() {
        return Err(DbError::config(
            "a role name is empty",
            "name the role: Principal::new().role(\"app_member\")",
        ));
    }
    if name.len() > MAX_IDENT {
        return Err(DbError::config(
            format!(
                "the role name `{name}` is {} bytes; Postgres keeps {MAX_IDENT}",
                name.len()
            ),
            "use a shorter name: Postgres would truncate it and name another role",
        ));
    }
    if name.contains('\0') {
        return Err(DbError::config(
            "a role name holds a NUL byte",
            "remove the NUL byte from the name",
        ));
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

/// A setting key: `[a-z][a-z0-9_]*`, at most 63 bytes with the prefix.
///
/// # Errors
/// [`DbError::Config`] for a key outside that shape.
pub fn check_key(key: &str) -> Result<(), DbError> {
    let mut chars = key.chars();
    let shaped = chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    if !shaped || SETTING_PREFIX.len() + key.len() > MAX_IDENT {
        return Err(DbError::config(
            format!("the principal key `{key}` is not a setting name"),
            "use lower-case letters, digits and `_`, starting with a letter: `tenant_id`",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_statements_bind_each_value_and_quote_the_role() {
        let p = Principal::new()
            .role("app \"member\"")
            .set("tenant_id", 7)
            .set("user_id", "x'; DROP TABLE posts; --");
        let Statements { role, settings } = p.statements().unwrap();
        assert_eq!(role.unwrap(), r#"SET LOCAL ROLE "app ""member""""#);
        let (sql, binds) = settings.unwrap();
        assert_eq!(
            sql,
            "SELECT set_config($1, $2, true), set_config($3, $4, true)"
        );
        assert_eq!(
            binds,
            [
                "brocanth.tenant_id",
                "7",
                "brocanth.user_id",
                "x'; DROP TABLE posts; --"
            ]
        );
    }

    #[test]
    fn a_second_value_for_a_key_replaces_the_first() {
        let p = Principal::new().set("tenant_id", 1).set("tenant_id", 2);
        assert_eq!(p.settings(), [("tenant_id".to_string(), "2".to_string())]);
    }

    #[test]
    fn a_bad_key_or_role_is_refused_with_help() {
        for p in [
            Principal::new().set("Tenant", 1),
            Principal::new().set("tenant-id", 1),
            Principal::new().set("", 1),
            Principal::new().role(""),
            Principal::new().role("r".repeat(64)),
        ] {
            let e = p.statements().unwrap_err().to_string();
            assert!(e.contains("\nhelp: "), "{e}");
        }
    }

    #[test]
    fn no_role_and_no_value_runs_nothing() {
        assert_eq!(
            Principal::new().statements().unwrap(),
            Statements {
                role: None,
                settings: None
            }
        );
    }
}
