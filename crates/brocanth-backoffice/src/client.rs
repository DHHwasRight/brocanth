//! The admin's end of the contract: an [`AdminData`] that sends every read
//! and write through a [`TabData`].

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::data::{
    self, AdminData, AdminTransaction, DataError, ListRequest, Locked, RequestCx, SharedData,
};
use brocanth_admin::discovery::RegistryEntry;
use brocanth_admin::model::{ModelDescriptor, PrimaryKey, Row, Value};
use brocanth_admin::policy::{Level, Principal};

use crate::contract::{
    Answer, Call, Caller, Claim, Equals, Op, Read, Scope, Search, SharedTabs, TabData, TabError,
    TabTransaction, Write,
};

/// How the admin's end turns the request's principal into a [`Claim`].
pub type ClaimFn = Arc<dyn for<'p> Fn(&Principal<'p>) -> Claim + Send + Sync>;

/// An [`AdminData`] over a [`TabData`]: the admin's end of the contract.
///
/// Each request gets a copy bound to the request's [`Claim`]
/// ([`AdminData::for_request`]), and every call it sends carries the claim,
/// the tab and the level. The admin gives it to `Admin::try_new` like any
/// source, or a project maps its source with
/// `AdminConfig::data_through(|data| Contracted::new(InProcess::new(data)).shared())`.
#[derive(Clone)]
pub struct Contracted {
    tabs: SharedTabs,
    claim_of: ClaimFn,
    claim: Claim,
}

impl std::fmt::Debug for Contracted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contracted")
            .field("claim", &self.claim)
            .finish_non_exhaustive()
    }
}

impl Contracted {
    /// The admin's end of `tabs`. The claim of a principal is
    /// [`Claim::from_principal`] until [`Contracted::claim`] says else.
    pub fn new(tabs: impl TabData) -> Contracted {
        Contracted::over(Arc::new(tabs))
    }

    /// [`Contracted::new`], over a data side that is already shared.
    pub fn over(tabs: SharedTabs) -> Contracted {
        Contracted {
            tabs,
            claim_of: Arc::new(|principal: &Principal<'_>| Claim::from_principal(principal)),
            claim: Claim::anonymous(),
        }
    }

    /// Builds each request's claim with `claim_of`: read the application's
    /// own principal type, such as `brocanth_auth::Staff`, so its grants
    /// travel.
    pub fn claim(
        mut self,
        claim_of: impl for<'p> Fn(&Principal<'p>) -> Claim + Send + Sync + 'static,
    ) -> Contracted {
        self.claim_of = Arc::new(claim_of);
        self
    }

    /// As the admin holds a source.
    pub fn shared(self) -> SharedData {
        Arc::new(self)
    }

    /// The claim this copy's calls carry: anonymous until
    /// [`AdminData::for_request`] binds one.
    pub fn current_claim(&self) -> &Claim {
        &self.claim
    }

    fn caller(&self, tab: &str, level: Level) -> Caller {
        Caller {
            tab: tab.to_string(),
            level,
            claim: self.claim.clone(),
        }
    }

    async fn ask(&self, op: Op) -> Result<Answer, DataError> {
        let caller = self.caller(op.model(), op.level());
        Ok(self.tabs.call(Call { caller, op }).await?)
    }
}

fn changed(op: &str, answer: Answer) -> Result<Locked, DataError> {
    match answer {
        Answer::Changed { rows, before } => Ok(Locked::new(rows, before)),
        other => Err(TabError::unexpected(op, &other).into()),
    }
}

fn key(answer: Answer) -> Result<PrimaryKey, DataError> {
    match answer {
        Answer::Key(key) => Ok(key),
        other => Err(TabError::unexpected("insert", &other).into()),
    }
}

fn insert(model: &ModelDescriptor, values: &[(String, Value)]) -> Write {
    Write::Insert {
        model: model.key.clone(),
        values: values.to_vec(),
    }
}

fn update(
    model: &ModelDescriptor,
    key: &PrimaryKey,
    values: &[(String, Value)],
    scope: data::Scope<'_>,
) -> Write {
    Write::Update {
        model: model.key.clone(),
        key: key.clone(),
        values: values.to_vec(),
        scope: Scope::of(scope),
    }
}

fn delete(model: &ModelDescriptor, key: &PrimaryKey, scope: data::Scope<'_>) -> Write {
    Write::Delete {
        model: model.key.clone(),
        key: key.clone(),
        scope: Scope::of(scope),
    }
}

#[async_trait]
impl AdminData for Contracted {
    fn for_request(&self, cx: &RequestCx<'_>) -> Option<SharedData> {
        let mut bound = self.clone();
        bound.claim = (self.claim_of)(&cx.principal);
        Some(Arc::new(bound))
    }

    fn check(&self) -> Result<(), DataError> {
        Ok(self.tabs.check()?)
    }

    fn registry(&self) -> Vec<RegistryEntry> {
        self.tabs.registry()
    }

    async fn count(
        &self,
        model: &ModelDescriptor,
        search: data::Search<'_>,
        scope: data::Scope<'_>,
    ) -> Result<usize, DataError> {
        let answer = self
            .ask(Op::Read(Read::Count {
                model: model.key.clone(),
                search: Search::of(search),
                scope: Scope::of(scope),
            }))
            .await?;
        match answer {
            Answer::Count(n) => usize::try_from(n).map_err(|_| {
                TabError::protocol(format!("a count of {n} rows does not fit this platform")).into()
            }),
            other => Err(TabError::unexpected("count", &other).into()),
        }
    }

    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        let answer = self
            .ask(Op::Read(Read::List {
                model: request.model.key.clone(),
                offset: request.offset as u64,
                limit: request.limit as u64,
                search: Search::of(request.search),
                sort: request.sort.cloned(),
                filter: request.filter.map(|filter| Equals {
                    column: filter.column.to_string(),
                    value: filter.value.clone(),
                }),
                scope: Scope::of(request.scope),
            }))
            .await?;
        match answer {
            Answer::Rows(rows) => Ok(rows),
            other => Err(TabError::unexpected("list", &other).into()),
        }
    }

    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: data::Scope<'_>,
    ) -> Result<Option<Row>, DataError> {
        let answer = self
            .ask(Op::Read(Read::Fetch {
                model: model.key.clone(),
                key: key.clone(),
                scope: Scope::of(scope),
            }))
            .await?;
        match answer {
            Answer::Row(row) => Ok(row),
            other => Err(TabError::unexpected("fetch", &other).into()),
        }
    }

    async fn fetch_many(
        &self,
        model: &ModelDescriptor,
        column: &str,
        keys: &[String],
        scope: data::Scope<'_>,
    ) -> Result<Vec<Row>, DataError> {
        let answer = self
            .ask(Op::Read(Read::FetchMany {
                model: model.key.clone(),
                column: column.to_string(),
                keys: keys.to_vec(),
                scope: Scope::of(scope),
            }))
            .await?;
        match answer {
            Answer::Rows(rows) => Ok(rows),
            other => Err(TabError::unexpected("fetch_many", &other).into()),
        }
    }

    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        key(self.ask(Op::Write(insert(model, values))).await?)
    }

    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: data::Scope<'_>,
    ) -> Result<u64, DataError> {
        Ok(self.update_locked(model, key, values, scope).await?.rows)
    }

    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: data::Scope<'_>,
    ) -> Result<u64, DataError> {
        Ok(self.delete_locked(model, key, scope).await?.rows)
    }

    async fn update_locked(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: data::Scope<'_>,
    ) -> Result<Locked, DataError> {
        let answer = self
            .ask(Op::Write(update(model, key, values, scope)))
            .await?;
        changed("update", answer)
    }

    async fn delete_locked(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: data::Scope<'_>,
    ) -> Result<Locked, DataError> {
        let answer = self.ask(Op::Write(delete(model, key, scope))).await?;
        changed("delete", answer)
    }

    async fn begin(&self) -> Result<Box<dyn AdminTransaction + '_>, DataError> {
        let tx = self.tabs.begin(&self.claim).await?;
        Ok(Box::new(ClientTransaction { owner: self, tx }))
    }
}

/// A transaction of the data side, as the admin writes through one.
struct ClientTransaction<'a> {
    owner: &'a Contracted,
    tx: Box<dyn TabTransaction>,
}

impl ClientTransaction<'_> {
    async fn ask(&mut self, write: Write) -> Result<Answer, DataError> {
        let caller = self.owner.caller(write.model(), Level::Edit);
        Ok(self.tx.call(caller, write).await?)
    }
}

#[async_trait]
impl AdminTransaction for ClientTransaction<'_> {
    async fn insert(
        &mut self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        key(self.ask(insert(model, values)).await?)
    }

    async fn update(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: data::Scope<'_>,
    ) -> Result<u64, DataError> {
        Ok(self.update_locked(model, key, values, scope).await?.rows)
    }

    async fn delete(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: data::Scope<'_>,
    ) -> Result<u64, DataError> {
        Ok(self.delete_locked(model, key, scope).await?.rows)
    }

    async fn update_locked(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: data::Scope<'_>,
    ) -> Result<Locked, DataError> {
        let answer = self.ask(update(model, key, values, scope)).await?;
        changed("update", answer)
    }

    async fn delete_locked(
        &mut self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: data::Scope<'_>,
    ) -> Result<Locked, DataError> {
        let answer = self.ask(delete(model, key, scope)).await?;
        changed("delete", answer)
    }

    async fn commit(self: Box<Self>) -> Result<(), DataError> {
        Ok(self.tx.commit().await?)
    }

    async fn rollback(self: Box<Self>) -> Result<(), DataError> {
        Ok(self.tx.rollback().await?)
    }
}
