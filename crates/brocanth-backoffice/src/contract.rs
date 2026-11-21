//! The tab data contract: what a model tab asks its data side, and what it
//! gets back, as owned serde types (docs/backoffice.md §7.1).
//!
//! A call is one [`Call`]: who asks ([`Caller`]) and what ([`Op`]). The
//! answer is one [`Answer`], or a [`TabError`]. Nothing borrows, so a call
//! can cross a process: [`crate::Json`] encodes each one and decodes it
//! again, and the admin's screens run through it in this crate's tests.
//!
//! [`TabData`] is the trait the data side implements. [`crate::InProcess`]
//! implements it over any [`AdminData`](brocanth_admin::AdminData), and
//! [`crate::Contracted`] is the admin's end: an `AdminData` that sends
//! every read and write through a `TabData`.

use std::fmt;

use async_trait::async_trait;
use brocanth_admin::data::{self, DataError, FieldErrors};
use brocanth_admin::discovery::RegistryEntry;
use brocanth_admin::model::{PrimaryKey, Row, Value};
use brocanth_admin::policy::{Level, Principal};
use brocanth_admin::query::Sort;
use brocanth_policy::Condition;
use serde::{Deserialize, Serialize};

/// Who a call is for, as the data side needs it: for RBAC and for the
/// database identity (D5).
///
/// The admin's end builds it from the request's [`Principal`] with a
/// function ([`crate::Contracted::claim`]). The data side gives
/// `Principal::new(&claim)` to the source's `for_request`, so a source that
/// runs as the staff member's database identity reads the claim:
/// `principal.downcast::<Claim>()`, or only `principal.is_anonymous()`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// The member of staff, as the application names them: an account id.
    /// `None` is nobody. An empty string is somebody the claim function
    /// could not name.
    pub subject: Option<String>,
    /// Holds every level on every tab.
    pub superuser: bool,
    /// Permission strings, `<resource>.<level>`: `post.edit`, `jobs.read`.
    pub grants: Vec<String>,
}

impl Claim {
    /// Nobody. Holds no level on any tab.
    pub fn anonymous() -> Claim {
        Claim::default()
    }

    /// A member of staff, with their permission strings.
    pub fn staff<I, S>(subject: impl Into<String>, superuser: bool, grants: I) -> Claim
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Claim {
            subject: Some(subject.into()),
            superuser,
            grants: grants.into_iter().map(Into::into).collect(),
        }
    }

    /// The claim of any principal, with no knowledge of its type: nobody
    /// for an anonymous one, the claim itself for a [`Claim`], and
    /// somebody with no grant for anything else.
    ///
    /// The default of [`crate::Contracted`]. A project with `superuser`
    /// reads its `Staff` instead, so the grants travel.
    pub fn from_principal(principal: &Principal<'_>) -> Claim {
        if principal.is_anonymous() {
            return Claim::anonymous();
        }
        match principal.downcast::<Claim>() {
            Some(claim) => claim.clone(),
            None => Claim {
                subject: Some(String::new()),
                ..Claim::default()
            },
        }
    }

    /// Whether this is nobody.
    pub fn is_anonymous(&self) -> bool {
        self.subject.is_none()
    }

    /// Whether the claim holds `level` on `tab`. A higher level holds the
    /// lower ones, and a superuser holds every level, as
    /// `brocanth_auth::Staff::allows` decides.
    pub fn holds(&self, tab: &str, level: Level) -> bool {
        if self.superuser {
            return true;
        }
        Level::ALL
            .into_iter()
            .filter(|held| *held >= level)
            .any(|held| self.grants.iter().any(|grant| *grant == held.on(tab)))
    }

    /// The principal the data side hands its source: anonymous for
    /// nobody, this claim otherwise.
    pub fn principal(&self) -> Principal<'_> {
        if self.is_anonymous() {
            Principal::anonymous()
        } else {
            Principal::new(self)
        }
    }
}

/// Who asks, and for what: the tab, which is the RBAC resource, and the
/// level the call needs on it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caller {
    /// The tab: for a model screen, the model's key.
    pub tab: String,
    /// `read` for a read, `edit` for a write.
    pub level: Level,
    /// Who.
    pub claim: Claim,
}

/// The row scope, owned: the condition the admin compiled from the model's
/// row policy (D4). See [`data::Scope`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Scope {
    /// No row rule applies.
    Unscoped,
    /// `AND` this condition into the `WHERE` clause.
    Where(Condition),
}

impl Scope {
    /// The admin's borrowed scope, owned.
    pub fn of(scope: data::Scope<'_>) -> Scope {
        match scope.condition() {
            Some(condition) => Scope::Where(condition.clone()),
            None => Scope::Unscoped,
        }
    }

    /// Borrowed again, as a source takes it.
    pub fn as_data(&self) -> data::Scope<'_> {
        match self {
            Scope::Unscoped => data::Scope::Unscoped,
            Scope::Where(condition) => data::Scope::Where(condition),
        }
    }
}

/// A search term and its columns, owned. See [`data::Search`].
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Search {
    /// The term. Empty matches every row.
    pub term: String,
    /// The columns it applies to.
    pub columns: Vec<String>,
}

impl Search {
    /// The admin's borrowed search, owned.
    pub fn of(search: data::Search<'_>) -> Search {
        Search {
            term: search.term.to_string(),
            columns: search.columns.to_vec(),
        }
    }

    /// Borrowed again, as a source takes it.
    pub fn as_data(&self) -> data::Search<'_> {
        data::Search {
            term: &self.term,
            columns: &self.columns,
        }
    }
}

/// `AND <column> = <value>`, owned. See [`data::Equals`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Equals {
    /// The column.
    pub column: String,
    /// The value it must hold.
    pub value: Value,
}

/// A read. Each names its model by key (D3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Read {
    /// How many rows match the search and the scope.
    Count {
        /// The model's key.
        model: String,
        /// The search.
        search: Search,
        /// The row scope.
        scope: Scope,
    },
    /// One page of rows.
    List {
        /// The model's key.
        model: String,
        /// Rows to skip.
        offset: u64,
        /// Rows to return.
        limit: u64,
        /// The search.
        search: Search,
        /// The sort, already checked against the model.
        sort: Option<Sort>,
        /// Only the rows where one column holds one value.
        filter: Option<Equals>,
        /// The row scope.
        scope: Scope,
    },
    /// One row by its key.
    Fetch {
        /// The model's key.
        model: String,
        /// The row's key.
        key: PrimaryKey,
        /// The row scope.
        scope: Scope,
    },
    /// The rows whose `column` holds one of `keys`.
    FetchMany {
        /// The model's key.
        model: String,
        /// The column.
        column: String,
        /// The values.
        keys: Vec<String>,
        /// The row scope.
        scope: Scope,
    },
}

/// A write. Each names its model by key (D3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Write {
    /// A new row.
    Insert {
        /// The model's key.
        model: String,
        /// The values, by column.
        values: Vec<(String, Value)>,
    },
    /// Over an existing row, guarded by the scope.
    Update {
        /// The model's key.
        model: String,
        /// The row's key.
        key: PrimaryKey,
        /// The values, by column.
        values: Vec<(String, Value)>,
        /// The row scope.
        scope: Scope,
    },
    /// Removes a row, guarded by the scope.
    Delete {
        /// The model's key.
        model: String,
        /// The row's key.
        key: PrimaryKey,
        /// The row scope.
        scope: Scope,
    },
}

impl Write {
    /// The model's key.
    pub fn model(&self) -> &str {
        match self {
            Write::Insert { model, .. }
            | Write::Update { model, .. }
            | Write::Delete { model, .. } => model,
        }
    }
}

impl Read {
    /// The model's key.
    pub fn model(&self) -> &str {
        match self {
            Read::Count { model, .. }
            | Read::List { model, .. }
            | Read::Fetch { model, .. }
            | Read::FetchMany { model, .. } => model,
        }
    }
}

/// What a call asks for.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Op {
    /// A read.
    Read(Read),
    /// A write outside a transaction.
    Write(Write),
}

impl Op {
    /// The level the operation needs on its tab: `read` for a read, `edit`
    /// for a write.
    pub fn level(&self) -> Level {
        match self {
            Op::Read(_) => Level::Read,
            Op::Write(_) => Level::Edit,
        }
    }

    /// The model's key.
    pub fn model(&self) -> &str {
        match self {
            Op::Read(read) => read.model(),
            Op::Write(write) => write.model(),
        }
    }

    /// The word for a message: `count`, `insert`.
    pub fn name(&self) -> &'static str {
        match self {
            Op::Read(Read::Count { .. }) => "count",
            Op::Read(Read::List { .. }) => "list",
            Op::Read(Read::Fetch { .. }) => "fetch",
            Op::Read(Read::FetchMany { .. }) => "fetch_many",
            Op::Write(Write::Insert { .. }) => "insert",
            Op::Write(Write::Update { .. }) => "update",
            Op::Write(Write::Delete { .. }) => "delete",
        }
    }
}

/// One call: who asks, and what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Call {
    /// Who asks.
    pub caller: Caller,
    /// What.
    pub op: Op,
}

/// What a call gets back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Answer {
    /// To a count.
    Count(u64),
    /// To a list or a fetch of many.
    Rows(Vec<Row>),
    /// To a fetch: `None` when the key matches nothing or the scope refuses
    /// the row. The two are one answer on purpose.
    Row(Option<Row>),
    /// To an insert: the new row's key.
    Key(PrimaryKey),
    /// To an update or a delete: the rows changed, and the row as the
    /// write read it under its lock, when the source read one.
    Changed {
        /// Rows changed. Zero is a failure, never success.
        rows: u64,
        /// The row before the write.
        before: Option<Row>,
    },
    /// To the end of a transaction.
    Done,
}

impl Answer {
    /// The word for a message: `count`, `rows`.
    pub fn name(&self) -> &'static str {
        match self {
            Answer::Count(_) => "count",
            Answer::Rows(_) => "rows",
            Answer::Row(_) => "row",
            Answer::Key(_) => "key",
            Answer::Changed { .. } => "changed",
            Answer::Done => "done",
        }
    }
}

/// What kind of failure a [`TabError`] is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    /// The data side holds no model with this key.
    UnknownModel {
        /// The key.
        model: String,
    },
    /// The record is not valid: the messages, by field.
    Invalid(FieldErrors),
    /// The claim does not hold the level the call needs on its tab.
    Refused,
    /// The storage failed.
    Backend,
    /// The two ends do not agree: an answer of the wrong kind, a call that
    /// does not decode, a transaction that already ended.
    Protocol,
}

/// A failure, as data: a kind, a message and the line that says what to do
/// next (D7).
///
/// It converts to [`DataError`] and back, so a form still shows each
/// message beside its field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabError {
    /// What kind of failure.
    pub kind: ErrorKind,
    /// What went wrong.
    pub message: String,
    /// What to do next, without the `help: ` prefix.
    pub help: Option<String>,
}

impl TabError {
    /// A failure of `kind`.
    pub fn new(kind: ErrorKind, message: impl Into<String>, help: impl Into<String>) -> TabError {
        TabError {
            kind,
            message: message.into(),
            help: Some(help.into()),
        }
    }

    /// The two ends do not agree.
    pub fn protocol(message: impl Into<String>) -> TabError {
        TabError::new(
            ErrorKind::Protocol,
            message,
            "build the admin and its data side from one version of brocanth-backoffice",
        )
    }

    /// The claim does not hold `level` on `tab`.
    pub fn refused(caller: &Caller) -> TabError {
        let who = match &caller.claim.subject {
            None => "nobody".to_string(),
            Some(subject) if subject.is_empty() => "an unnamed member of staff".to_string(),
            Some(subject) => format!("`{subject}`"),
        };
        TabError::new(
            ErrorKind::Refused,
            format!(
                "the data side refused a call on `{}`: {who} does not hold `{}`",
                caller.tab,
                caller.level.on(&caller.tab)
            ),
            format!(
                "grant `{}` to a role of this member of staff in the roles tab, or check that \
                 the admin's policy and the claim function read the same grants",
                caller.level.on(&caller.tab)
            ),
        )
    }

    /// An answer of the wrong kind to `op`.
    pub fn unexpected(op: &str, answer: &Answer) -> TabError {
        TabError::protocol(format!(
            "the data side answered `{}` to `{op}`",
            answer.name()
        ))
    }
}

impl fmt::Display for TabError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(help) = &self.help {
            write!(f, "\nhelp: {help}")?;
        }
        Ok(())
    }
}

impl std::error::Error for TabError {}

impl From<DataError> for TabError {
    fn from(error: DataError) -> TabError {
        match error {
            DataError::UnknownModel(model) => TabError {
                message: format!("no model named `{model}`"),
                kind: ErrorKind::UnknownModel { model },
                help: Some("list the model in the data source's registry".to_string()),
            },
            DataError::Invalid(fields) => TabError {
                message: "the record is not valid".to_string(),
                kind: ErrorKind::Invalid(fields),
                help: None,
            },
            DataError::Backend(text) => {
                let (message, help) = match text.rsplit_once("\nhelp: ") {
                    Some((message, help)) => (message.to_string(), Some(help.to_string())),
                    None => (text, None),
                };
                TabError {
                    kind: ErrorKind::Backend,
                    message,
                    help,
                }
            }
        }
    }
}

impl From<TabError> for DataError {
    fn from(error: TabError) -> DataError {
        match error.kind {
            ErrorKind::UnknownModel { model } => DataError::UnknownModel(model),
            ErrorKind::Invalid(fields) => DataError::Invalid(fields),
            ErrorKind::Refused | ErrorKind::Backend | ErrorKind::Protocol => {
                DataError::Backend(error.to_string())
            }
        }
    }
}

/// The data side of one kind of tab: the admin's model screens.
///
/// Implemented in one process by [`crate::InProcess`]. A later transport
/// implements it as a client of a server that holds an `InProcess`; the
/// types do not change.
#[async_trait]
pub trait TabData: Send + Sync + 'static {
    /// Every model the data side holds. Read once, when the admin is
    /// built: an entity registry does not change while a process runs.
    fn registry(&self) -> Vec<RegistryEntry>;

    /// Refuses a configuration the data side cannot serve a call with,
    /// before it serves one. `Admin::try_new` runs it, so a generated
    /// project fails at start.
    ///
    /// # Errors
    ///
    /// The problem, with a `help:` line.
    fn check(&self) -> Result<(), TabError> {
        Ok(())
    }

    /// Runs one call.
    ///
    /// # Errors
    ///
    /// [`TabError`]: a refused claim, an unknown model, an invalid record,
    /// a storage failure.
    async fn call(&self, call: Call) -> Result<Answer, TabError>;

    /// Opens a unit of work for `claim`: writes kept all together or not
    /// at all. The transaction borrows nothing (D8).
    ///
    /// # Errors
    ///
    /// When the storage cannot open one.
    async fn begin(&self, claim: &Claim) -> Result<Box<dyn TabTransaction>, TabError>;
}

/// The writes of one unit of work, from [`TabData::begin`]. Dropped
/// without [`TabTransaction::commit`], it keeps nothing.
#[async_trait]
pub trait TabTransaction: Send {
    /// One write, under the same rules as [`TabData::call`].
    ///
    /// # Errors
    ///
    /// As [`TabData::call`].
    async fn call(&mut self, caller: Caller, write: Write) -> Result<Answer, TabError>;

    /// Keeps every write. An error means none is kept.
    ///
    /// # Errors
    ///
    /// When the storage cannot commit.
    async fn commit(self: Box<Self>) -> Result<(), TabError>;

    /// Discards every write.
    ///
    /// # Errors
    ///
    /// When the storage cannot roll back.
    async fn rollback(self: Box<Self>) -> Result<(), TabError>;
}

/// A shared data side, which is how [`crate::Contracted`] holds one.
pub type SharedTabs = std::sync::Arc<dyn TabData>;
