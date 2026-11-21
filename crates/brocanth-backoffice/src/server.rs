//! The data side of the contract in one process: a [`TabData`] over any
//! [`AdminData`].

use std::future::{poll_fn, Future};
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::Poll;

use async_trait::async_trait;
use brocanth_admin::data::{
    AdminData, AdminTransaction, Equals, ListRequest, RequestCx, SharedData,
};
use brocanth_admin::discovery::{discover, Discovery, RegistryEntry};
use brocanth_admin::model::ModelDescriptor;

use crate::contract::{
    Answer, Call, Caller, Claim, ErrorKind, Op, Read, TabData, TabError, TabTransaction, Write,
};

/// A [`TabData`] over an [`AdminData`]: the data side, in the process that
/// holds the source.
///
/// For each call it builds the principal from the call's [`Claim`]
/// ([`Claim::principal`]) and asks the source's `for_request` for the
/// source to use, so a source with `as_identity` runs the call as the staff
/// member's database identity (D5). It finds the model by key in the
/// source's own registry (D3).
pub struct InProcess {
    source: SharedData,
    models: Vec<ModelDescriptor>,
    enforce: bool,
}

impl std::fmt::Debug for InProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InProcess")
            .field(
                "models",
                &self
                    .models
                    .iter()
                    .map(|m| m.key.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("enforce", &self.enforce)
            .finish_non_exhaustive()
    }
}

impl InProcess {
    /// The data side of `source`. Grants are not checked: see
    /// [`InProcess::enforcing_grants`].
    pub fn new(source: SharedData) -> InProcess {
        // Every model the source holds, the migration table too: the
        // admin's discovery decides what it shows, and the data side
        // serves what it holds.
        let models = discover(
            source.registry(),
            &Discovery::new().keeping_migration_tables(),
        );
        InProcess {
            source,
            models,
            enforce: false,
        }
    }

    /// Refuses a call whose claim does not hold `<tab>.<level>` (D6).
    ///
    /// The admin's policy has already decided the screen when the call
    /// arrives, so in one process this repeats it. A server that takes
    /// calls from another process turns it on: it must not trust the
    /// caller's decision.
    pub fn enforcing_grants(mut self) -> InProcess {
        self.enforce = true;
        self
    }

    fn admit(&self, caller: &Caller) -> Result<(), TabError> {
        if self.enforce && !caller.claim.holds(&caller.tab, caller.level) {
            return Err(TabError::refused(caller));
        }
        Ok(())
    }

    fn model(&self, key: &str) -> Result<&ModelDescriptor, TabError> {
        self.models.iter().find(|m| m.key == key).ok_or_else(|| {
            TabError::new(
                ErrorKind::UnknownModel {
                    model: key.to_string(),
                },
                format!("the data side holds no model with the key `{key}`"),
                "list the model in the data source, or build the admin and its data side \
                     from one version of the application",
            )
        })
    }

    /// The source for `claim`: bound to its identity, or the shared one.
    fn source_for(&self, claim: &Claim) -> SharedData {
        self.source
            .for_request(&RequestCx::new(claim.principal()))
            .unwrap_or_else(|| Arc::clone(&self.source))
    }

    async fn read(&self, source: &dyn AdminData, read: Read) -> Result<Answer, TabError> {
        match read {
            Read::Count {
                model,
                search,
                scope,
            } => {
                let model = self.model(&model)?;
                let n = source
                    .count(model, search.as_data(), scope.as_data())
                    .await?;
                Ok(Answer::Count(n as u64))
            }
            Read::List {
                model,
                offset,
                limit,
                search,
                sort,
                filter,
                scope,
            } => {
                let model = self.model(&model)?;
                let too_big = |what: &str| {
                    TabError::protocol(format!("a list {what} that does not fit this platform"))
                };
                let request = ListRequest {
                    model,
                    offset: usize::try_from(offset).map_err(|_| too_big("offset"))?,
                    limit: usize::try_from(limit).map_err(|_| too_big("limit"))?,
                    search: search.as_data(),
                    sort: sort.as_ref(),
                    filter: filter.as_ref().map(|filter| Equals {
                        column: &filter.column,
                        value: &filter.value,
                    }),
                    scope: scope.as_data(),
                };
                Ok(Answer::Rows(source.list(request).await?))
            }
            Read::Fetch { model, key, scope } => {
                let model = self.model(&model)?;
                Ok(Answer::Row(
                    source.fetch(model, &key, scope.as_data()).await?,
                ))
            }
            Read::FetchMany {
                model,
                column,
                keys,
                scope,
            } => {
                let model = self.model(&model)?;
                let rows = source
                    .fetch_many(model, &column, &keys, scope.as_data())
                    .await?;
                Ok(Answer::Rows(rows))
            }
        }
    }

    async fn write(&self, source: &dyn AdminData, write: Write) -> Result<Answer, TabError> {
        match write {
            Write::Insert { model, values } => {
                let model = self.model(&model)?;
                Ok(Answer::Key(source.insert(model, &values).await?))
            }
            Write::Update {
                model,
                key,
                values,
                scope,
            } => {
                let model = self.model(&model)?;
                let locked = source
                    .update_locked(model, &key, &values, scope.as_data())
                    .await?;
                Ok(Answer::Changed {
                    rows: locked.rows,
                    before: locked.before,
                })
            }
            Write::Delete { model, key, scope } => {
                let model = self.model(&model)?;
                let locked = source.delete_locked(model, &key, scope.as_data()).await?;
                Ok(Answer::Changed {
                    rows: locked.rows,
                    before: locked.before,
                })
            }
        }
    }
}

/// [`InProcess::write`], on a transaction.
async fn write_in(
    models: &[ModelDescriptor],
    tx: &mut (dyn AdminTransaction + '_),
    write: Write,
) -> Result<Answer, TabError> {
    let model = |key: &str| {
        models.iter().find(|m| m.key == key).ok_or_else(|| {
            TabError::new(
                ErrorKind::UnknownModel {
                    model: key.to_string(),
                },
                format!("the data side holds no model with the key `{key}`"),
                "list the model in the data source",
            )
        })
    };
    match write {
        Write::Insert { model: key, values } => {
            Ok(Answer::Key(tx.insert(model(&key)?, &values).await?))
        }
        Write::Update {
            model: key,
            key: pk,
            values,
            scope,
        } => {
            let locked = tx
                .update_locked(model(&key)?, &pk, &values, scope.as_data())
                .await?;
            Ok(Answer::Changed {
                rows: locked.rows,
                before: locked.before,
            })
        }
        Write::Delete {
            model: key,
            key: pk,
            scope,
        } => {
            let locked = tx.delete_locked(model(&key)?, &pk, scope.as_data()).await?;
            Ok(Answer::Changed {
                rows: locked.rows,
                before: locked.before,
            })
        }
    }
}

#[async_trait]
impl TabData for InProcess {
    fn registry(&self) -> Vec<RegistryEntry> {
        self.source.registry()
    }

    fn check(&self) -> Result<(), TabError> {
        Ok(self.source.check()?)
    }

    async fn call(&self, call: Call) -> Result<Answer, TabError> {
        let Call { caller, op } = call;
        if caller.tab != op.model() || caller.level < op.level() {
            return Err(TabError::protocol(format!(
                "a call names the tab `{}` at `{}`, and its `{}` needs `{}` on `{}`",
                caller.tab,
                caller.level,
                op.name(),
                op.level(),
                op.model()
            )));
        }
        self.admit(&caller)?;
        let source = self.source_for(&caller.claim);
        match op {
            Op::Read(read) => self.read(&*source, read).await,
            Op::Write(write) => self.write(&*source, write).await,
        }
    }

    async fn begin(&self, claim: &Claim) -> Result<Box<dyn TabTransaction>, TabError> {
        let source = self.source_for(claim);
        let models = self.models.clone();
        let mut tx = Owned::open(source, models);
        tx.ask(Step::Open).await?;
        Ok(Box::new(Transaction {
            tx,
            enforce: self.enforce,
        }))
    }
}

/// A transaction of the data side, as [`TabData::begin`] returns it.
struct Transaction {
    tx: Owned,
    enforce: bool,
}

#[async_trait]
impl TabTransaction for Transaction {
    async fn call(&mut self, caller: Caller, write: Write) -> Result<Answer, TabError> {
        if caller.tab != write.model() {
            return Err(TabError::protocol(format!(
                "a call names the tab `{}`, and its write is on `{}`",
                caller.tab,
                write.model()
            )));
        }
        if self.enforce && !caller.claim.holds(&caller.tab, crate::Level::Edit) {
            return Err(TabError::refused(&caller));
        }
        self.tx.ask(Step::Write(write)).await
    }

    async fn commit(mut self: Box<Self>) -> Result<(), TabError> {
        self.tx.ask(Step::Commit).await.map(drop)
    }

    async fn rollback(mut self: Box<Self>) -> Result<(), TabError> {
        self.tx.ask(Step::Rollback).await.map(drop)
    }
}

/// What the owned transaction is asked to do next.
enum Step {
    /// Open the source's transaction.
    Open,
    /// One write.
    Write(Write),
    /// Keep every write, and end.
    Commit,
    /// Discard every write, and end.
    Rollback,
}

/// The step the driver sent, and the answer the transaction gave.
#[derive(Default)]
struct Mailbox {
    step: Option<Step>,
    answer: Option<Result<Answer, TabError>>,
}

type Shared = Arc<Mutex<Mailbox>>;

fn lock(mail: &Shared) -> std::sync::MutexGuard<'_, Mailbox> {
    mail.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A source's transaction that borrows nothing (D8).
///
/// `AdminData::begin` returns a transaction that borrows its source, and
/// the source for a claim is a new `Arc` that the call that opened the
/// transaction holds. So the source and its transaction live together in
/// one pinned async block, which may borrow from itself. Each step is put
/// in the mailbox, and the block is polled, with the caller's waker, until
/// it puts the answer there. No runtime and no thread: the block runs
/// inside the caller's own poll. Dropped, the block drops the source's
/// transaction, which keeps nothing.
struct Owned {
    mail: Shared,
    body: Pin<Box<dyn Future<Output = ()> + Send>>,
    ended: bool,
}

impl Owned {
    fn open(source: SharedData, models: Vec<ModelDescriptor>) -> Owned {
        let mail: Shared = Arc::default();
        let inbox = Arc::clone(&mail);
        let body = async move {
            let step = next(&inbox).await;
            if !matches!(step, Step::Open) {
                reply(
                    &inbox,
                    Err(TabError::protocol(
                        "a transaction was used before it opened",
                    )),
                );
                return;
            }
            let mut tx = match source.begin().await {
                Ok(tx) => {
                    reply(&inbox, Ok(Answer::Done));
                    tx
                }
                Err(error) => {
                    reply(&inbox, Err(error.into()));
                    return;
                }
            };
            loop {
                match next(&inbox).await {
                    Step::Open => {
                        reply(
                            &inbox,
                            Err(TabError::protocol("a transaction was opened twice")),
                        );
                    }
                    Step::Write(write) => {
                        let answer = write_in(&models, &mut *tx, write).await;
                        reply(&inbox, answer);
                    }
                    Step::Commit => {
                        let done = tx.commit().await.map(|()| Answer::Done);
                        reply(&inbox, done.map_err(Into::into));
                        return;
                    }
                    Step::Rollback => {
                        let done = tx.rollback().await.map(|()| Answer::Done);
                        reply(&inbox, done.map_err(Into::into));
                        return;
                    }
                }
            }
        };
        Owned {
            mail,
            body: Box::pin(body),
            ended: false,
        }
    }

    /// Sends one step and waits for its answer.
    async fn ask(&mut self, step: Step) -> Result<Answer, TabError> {
        if self.ended {
            return Err(TabError::protocol("a transaction was used after it ended"));
        }
        lock(&self.mail).step = Some(step);
        poll_fn(|cx| {
            if !self.ended && self.body.as_mut().poll(cx).is_ready() {
                self.ended = true;
            }
            if let Some(answer) = lock(&self.mail).answer.take() {
                return Poll::Ready(answer);
            }
            if self.ended {
                return Poll::Ready(Err(TabError::protocol(
                    "a transaction ended before it answered",
                )));
            }
            Poll::Pending
        })
        .await
    }
}

/// The next step, once the driver has put one in the mailbox. The driver
/// polls the block right after it puts a step there, so this registers no
/// waker of its own.
async fn next(mail: &Shared) -> Step {
    poll_fn(|_| match lock(mail).step.take() {
        Some(step) => Poll::Ready(step),
        None => Poll::Pending,
    })
    .await
}

fn reply(mail: &Shared, answer: Result<Answer, TabError>) {
    lock(mail).answer = Some(answer);
}
