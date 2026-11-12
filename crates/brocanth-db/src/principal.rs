//! [`DbIdentity`]: who a transaction runs as, for Postgres row-level
//! security.
//!
//! A policy reads the identity from the transaction:
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
//!
//! The name is `DbIdentity`, not `Principal`: `brocanth_admin::Principal`
//! is the signed-in user of a request. A request maps its principal to a
//! `DbIdentity` for the database.

use std::collections::BTreeSet;
use std::fmt;

use crate::error::DbError;

/// The prefix of every setting a [`DbIdentity`] writes. A policy reads
/// `current_setting('brocanth.tenant_id', true)`.
pub const SETTING_PREFIX: &str = "brocanth.";

/// The longest identifier Postgres keeps. A longer one is truncated, so
/// `SET ROLE` would name another role.
pub const MAX_IDENT: usize = 63;

/// A typed setting key, for [`DbIdentity::setting`]. The policy that reads
/// the value and the request that writes it share one constant, so a typo
/// cannot pass. `brocanth_rls::Setting` implements it.
pub trait SettingKey {
    /// The key, without the `brocanth.` prefix.
    fn key(&self) -> &str;

    /// The SQL type a policy reads the value as, for an error message.
    fn sql_type(&self) -> &str;

    /// Whether a policy can read `value` as [`SettingKey::sql_type`].
    /// `true` when the type has no check.
    fn accepts(&self, value: &str) -> bool {
        let _ = value;
        true
    }
}

/// Who a transaction runs as: a role, and named values.
///
/// ```
/// use brocanth_db::DbIdentity;
///
/// let me = DbIdentity::new().role("app_member").set("tenant_id", 7).set("user_id", 42);
/// assert_eq!(me.settings().len(), 2);
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DbIdentity {
    role: Option<String>,
    settings: Vec<(String, String)>,
    /// A value a typed key refused, reported when the identity is assumed:
    /// a builder cannot return a `Result` at each step.
    problem: Option<(String, String)>,
}

impl DbIdentity {
    /// No role and no value: the transaction keeps the login role.
    pub fn new() -> DbIdentity {
        DbIdentity::default()
    }

    /// `SET LOCAL ROLE "<name>"`. The login role must be a member of it.
    pub fn role(mut self, name: impl Into<String>) -> DbIdentity {
        self.role = Some(name.into());
        self
    }

    /// `set_config('brocanth.<key>', '<value>', true)`. A second value for a
    /// key replaces the first.
    ///
    /// Prefer [`DbIdentity::setting`] with the constant the policy reads.
    pub fn set(mut self, key: impl Into<String>, value: impl fmt::Display) -> DbIdentity {
        let key = key.into();
        let value = value.to_string();
        match self.settings.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => self.settings.push((key, value)),
        }
        self
    }

    /// [`DbIdentity::set`] through a typed key: the key is the one the
    /// policy reads, and a value the policy cannot read as its type is
    /// refused when the identity is assumed.
    ///
    /// ```
    /// use brocanth_db::principal::SettingKey;
    /// use brocanth_db::DbIdentity;
    ///
    /// struct Tenant;
    /// impl SettingKey for Tenant {
    ///     fn key(&self) -> &str { "tenant_id" }
    ///     fn sql_type(&self) -> &str { "bigint" }
    ///     fn accepts(&self, value: &str) -> bool { value.parse::<i64>().is_ok() }
    /// }
    ///
    /// assert!(DbIdentity::new().setting(&Tenant, 7).statements().is_ok());
    /// assert!(DbIdentity::new().setting(&Tenant, "seven").statements().is_err());
    /// ```
    pub fn setting(self, key: &impl SettingKey, value: impl fmt::Display) -> DbIdentity {
        let value = value.to_string();
        let refused = !key.accepts(&value);
        let mut out = self.set(key.key(), &value);
        if refused && out.problem.is_none() {
            out.problem = Some((
                format!(
                    "the value `{value}` for `{}` is not a `{}`",
                    key.key(),
                    key.sql_type()
                ),
                format!(
                    "set a value the policy can read as `{}`: it casts the setting, and a bad cast fails the query",
                    key.sql_type()
                ),
            ));
        }
        out
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

    /// The statements a fresh transaction runs to take this identity: an
    /// optional `SET LOCAL ROLE`, and one `SELECT set_config(…)` with its
    /// binds (`brocanth.<key>`, value, in pairs). For a connection this
    /// crate does not hold, such as a SeaORM transaction.
    ///
    /// # Errors
    /// [`DbError::Config`] for a role name, a key or a value this crate
    /// refuses.
    pub fn statements(&self) -> Result<Statements, DbError> {
        self.statements_after(&Assumed::default())
    }

    /// The statements that replace `before` with this identity: a key
    /// `before` set and this identity does not is set to `''` (read as
    /// `NULL`), and a role `before` took is left with `SET LOCAL ROLE NONE`.
    pub(crate) fn statements_after(&self, before: &Assumed) -> Result<Statements, DbError> {
        if let Some((message, help)) = &self.problem {
            return Err(DbError::config(message.clone(), help.clone()));
        }
        let role = match &self.role {
            Some(name) => Some(format!("SET LOCAL ROLE {}", quote_ident(name)?)),
            None if before.role => Some("SET LOCAL ROLE NONE".to_string()),
            None => None,
        };
        let mut pairs: Vec<(&str, &str)> = Vec::new();
        for (key, value) in &self.settings {
            check_key(key)?;
            pairs.push((key, value));
        }
        for key in &before.keys {
            if !self.settings.iter().any(|(k, _)| k == key) {
                pairs.push((key, ""));
            }
        }
        if pairs.is_empty() {
            return Ok(Statements {
                role,
                settings: None,
            });
        }
        let mut sql = String::from("SELECT ");
        let mut binds = Vec::with_capacity(pairs.len() * 2);
        for (index, (key, value)) in pairs.into_iter().enumerate() {
            if index > 0 {
                sql.push_str(", ");
            }
            let n = index * 2 + 1;
            sql.push_str(&format!("set_config(${n}, ${}, true)", n + 1));
            binds.push(format!("{SETTING_PREFIX}{key}"));
            binds.push(value.to_string());
        }
        Ok(Statements {
            role,
            settings: Some((sql, binds)),
        })
    }
}

/// What a transaction runs to take a [`DbIdentity`].
#[derive(Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Statements {
    /// `SET LOCAL ROLE "<name>"` (or `NONE`), when the role changes. No
    /// bind parameter: `SET` takes none, so the name is a checked, quoted
    /// identifier.
    pub role: Option<String>,
    /// `SELECT set_config($1, $2, true), …` and its binds, when a value
    /// changes.
    pub settings: Option<(String, Vec<String>)>,
}

/// What earlier [`Tx::assume`](crate::Tx::assume) calls in one transaction
/// set, so the next call replaces it and does not merge with it.
#[derive(Debug, Clone, Default)]
pub(crate) struct Assumed {
    pub(crate) role: bool,
    pub(crate) keys: BTreeSet<String>,
}

impl Assumed {
    pub(crate) fn record(&mut self, identity: &DbIdentity) {
        self.role |= identity.role.is_some();
        self.keys
            .extend(identity.settings.iter().map(|(k, _)| k.clone()));
    }
}

/// Checks an identifier: not empty, at most 63 bytes, no NUL and no
/// control character. `what` names it in the message: `role`, `table`.
///
/// The one checker for identifiers this workspace writes into SQL;
/// `brocanth-rls` calls it too.
///
/// # Errors
/// [`DbError::Config`] naming the problem, with a `help:` line.
pub fn check_ident(what: &str, name: &str) -> Result<(), DbError> {
    if name.is_empty() {
        return Err(DbError::config(
            format!("a {what} name is empty"),
            format!("give the {what} a name"),
        ));
    }
    if name.len() > MAX_IDENT {
        return Err(DbError::config(
            format!(
                "the {what} name `{name}` is {} bytes; Postgres keeps {MAX_IDENT}",
                name.len()
            ),
            "use a shorter name: Postgres truncates a longer one, and it would name another object",
        ));
    }
    if name.chars().any(char::is_control) {
        return Err(DbError::config(
            format!("the {what} name {name:?} holds a control character"),
            "use letters, digits and `_`: a line break or a NUL byte has no place in a name",
        ));
    }
    Ok(())
}

/// `"name"`, with each `"` doubled, after [`check_ident`].
///
/// # Errors
/// What [`check_ident`] refuses.
pub fn quote_ident(name: &str) -> Result<String, DbError> {
    check_ident("role", name)?;
    Ok(double_quote(name))
}

/// `"name"`, with each `"` doubled. No check: call [`check_ident`] first.
pub fn double_quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
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
            format!("the identity key `{key}` is not a setting name"),
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
        let p = DbIdentity::new()
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
        let p = DbIdentity::new().set("tenant_id", 1).set("tenant_id", 2);
        assert_eq!(p.settings(), [("tenant_id".to_string(), "2".to_string())]);
    }

    #[test]
    fn a_bad_key_or_role_is_refused_with_help() {
        for p in [
            DbIdentity::new().set("Tenant", 1),
            DbIdentity::new().set("tenant-id", 1),
            DbIdentity::new().set("", 1),
            DbIdentity::new().role(""),
            DbIdentity::new().role("r".repeat(64)),
            DbIdentity::new().role("a\nb"),
        ] {
            let e = p.statements().unwrap_err().to_string();
            assert!(e.contains("\nhelp: "), "{e}");
        }
    }

    #[test]
    fn no_role_and_no_value_runs_nothing() {
        assert_eq!(
            DbIdentity::new().statements().unwrap(),
            Statements {
                role: None,
                settings: None
            }
        );
    }

    #[test]
    fn a_second_identity_clears_what_the_first_set() {
        let first = DbIdentity::new()
            .role("staff")
            .set("tenant_id", 1)
            .set("user_id", 2);
        let mut before = Assumed::default();
        before.record(&first);
        let second = DbIdentity::new().set("tenant_id", 3);
        let Statements { role, settings } = second.statements_after(&before).unwrap();
        assert_eq!(role.as_deref(), Some("SET LOCAL ROLE NONE"));
        let (_, binds) = settings.unwrap();
        assert_eq!(binds, ["brocanth.tenant_id", "3", "brocanth.user_id", ""]);
    }

    #[test]
    fn a_control_character_is_refused_in_any_name() {
        for name in ["a\nb", "a\rb", "a\0b", "a\tb"] {
            let e = check_ident("table", name).unwrap_err().to_string();
            assert!(e.contains("control character"), "{e}");
            assert!(e.contains("\nhelp: "), "{e}");
        }
        check_ident("table", "posts").unwrap();
    }
}
