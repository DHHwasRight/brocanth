//! [`Snapshot`]: the definitions as data, the content of
//! `migrations/rls.json`.
//!
//! The diff compares two snapshots. Expressions are stored as the SQL text
//! this crate wrote, so the compare is text against text, with no Postgres
//! normalization in between (`docs/rls.md` §3).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::ident::SqlType;

/// The snapshot format. A file with another version is refused.
pub const VERSION: u32 = 1;

/// Every definition a schema manages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// [`VERSION`].
    pub version: u32,
    /// Managed roles by name. A `Role::existing` is not here: this crate
    /// never creates or drops it.
    #[serde(default)]
    pub roles: BTreeMap<String, RoleSpec>,
    /// Functions by key (`schema.name` or `name`).
    #[serde(default)]
    pub functions: BTreeMap<String, FunctionSpec>,
    /// Tables by key, with their grants and policies.
    #[serde(default)]
    pub tables: BTreeMap<String, TableSpec>,
}

impl Default for Snapshot {
    fn default() -> Snapshot {
        Snapshot {
            version: VERSION,
            roles: BTreeMap::new(),
            functions: BTreeMap::new(),
            tables: BTreeMap::new(),
        }
    }
}

impl Snapshot {
    /// The snapshot as `migrations/rls.json` holds it: pretty JSON and a
    /// final newline.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("a snapshot serializes");
        text.push('\n');
        text
    }

    /// Reads `migrations/rls.json`.
    ///
    /// # Errors
    /// The parse error, or a message for another version.
    pub fn from_json(text: &str) -> Result<Snapshot, serde_json::Error> {
        let snapshot: Snapshot = serde_json::from_str(text)?;
        if snapshot.version != VERSION {
            return Err(serde::de::Error::custom(format!(
                "rls.json has version {}; this brocanth-rls reads version {VERSION}",
                snapshot.version
            )));
        }
        Ok(snapshot)
    }
}

/// A role this crate creates: `NOLOGIN`, never `SUPERUSER` or `BYPASSRLS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleSpec {
    /// `INHERIT` (the default) or `NOINHERIT`.
    pub inherit: bool,
    /// `CREATEDB`.
    pub createdb: bool,
    /// `CREATEROLE`.
    pub createrole: bool,
    /// The roles granted this one: `GRANT "role" TO "member"`.
    #[serde(default)]
    pub members: BTreeSet<String>,
}

/// `STABLE` is right for a function that reads a setting or a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Volatility {
    /// `IMMUTABLE`.
    Immutable,
    /// `STABLE`, the default.
    Stable,
    /// `VOLATILE`.
    Volatile,
}

impl Volatility {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Volatility::Immutable => "IMMUTABLE",
            Volatility::Stable => "STABLE",
            Volatility::Volatile => "VOLATILE",
        }
    }
}

/// A `LANGUAGE sql` function whose body is one `SELECT <expression>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionSpec {
    /// The schema, when it is not the first of `search_path`.
    pub schema: Option<String>,
    /// The name.
    pub name: String,
    /// The argument types, read in the body as `$1`, `$2`, ….
    pub args: Vec<SqlType>,
    /// The result type.
    pub returns: SqlType,
    /// `STABLE` unless set.
    pub volatility: Volatility,
    /// `SECURITY DEFINER SET search_path = …` when set.
    pub definer: Option<Vec<String>>,
    /// The expression after `SELECT`.
    pub body: String,
    /// The functions the body calls, by key.
    #[serde(default)]
    pub uses: BTreeSet<String>,
}

/// A table with row-level security on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSpec {
    /// The schema, when it is not the first of `search_path`.
    pub schema: Option<String>,
    /// The name.
    pub name: String,
    /// `FORCE ROW LEVEL SECURITY`: the owner obeys the policies too.
    pub force: bool,
    /// Table privileges by grantee (`PUBLIC` or a role name).
    #[serde(default)]
    pub grants: BTreeMap<String, BTreeSet<Privilege>>,
    /// Policies by name.
    #[serde(default)]
    pub policies: BTreeMap<String, PolicySpec>,
}

/// A table privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Privilege {
    /// `SELECT`.
    Select,
    /// `INSERT`.
    Insert,
    /// `UPDATE`.
    Update,
    /// `DELETE`.
    Delete,
}

impl Privilege {
    /// The four row privileges.
    pub const CRUD: [Privilege; 4] = [
        Privilege::Select,
        Privilege::Insert,
        Privilege::Update,
        Privilege::Delete,
    ];

    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Privilege::Select => "SELECT",
            Privilege::Insert => "INSERT",
            Privilege::Update => "UPDATE",
            Privilege::Delete => "DELETE",
        }
    }
}

/// `PERMISSIVE` policies are OR-ed together; a `RESTRICTIVE` one is AND-ed
/// with the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum PolicyKind {
    /// `AS PERMISSIVE`, the default.
    Permissive,
    /// `AS RESTRICTIVE`.
    Restrictive,
}

/// The command a policy applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Command {
    /// `FOR ALL`, the default.
    All,
    /// `FOR SELECT`: `USING` only.
    Select,
    /// `FOR INSERT`: `WITH CHECK` only.
    Insert,
    /// `FOR UPDATE`: `USING` for the old row, `WITH CHECK` for the new one.
    Update,
    /// `FOR DELETE`: `USING` only.
    Delete,
}

impl Command {
    pub(crate) fn as_sql(self) -> &'static str {
        match self {
            Command::All => "ALL",
            Command::Select => "SELECT",
            Command::Insert => "INSERT",
            Command::Update => "UPDATE",
            Command::Delete => "DELETE",
        }
    }
}

/// One policy, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicySpec {
    /// `AS PERMISSIVE` or `AS RESTRICTIVE`.
    #[serde(rename = "as")]
    pub kind: PolicyKind,
    /// `FOR …`.
    #[serde(rename = "for")]
    pub command: Command,
    /// `TO …`, sorted: role names, or `public`, `current_user`,
    /// `current_role`, `session_user`.
    pub to: Vec<String>,
    /// `USING (…)`.
    pub using: Option<String>,
    /// `WITH CHECK (…)`.
    pub with_check: Option<String>,
    /// The functions the expressions call, by key.
    #[serde(default)]
    pub uses: BTreeSet<String>,
}

/// The `TO` keywords that are not role names.
pub const KEYWORDS: [&str; 4] = ["public", "current_user", "current_role", "session_user"];

impl fmt::Display for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_sql())
    }
}
