//! The journal's "before" is the row the write locked, when the source
//! reads one, and not the row the admin read a moment earlier.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use brocanth_admin::data::{AdminTransaction, Locked};
use brocanth_admin::journal::AdminWrite;
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, AdminData, DataError, Discovery, Fixture, ListRequest, ModelDescriptor,
    PrimaryKey, Principal, RegistryEntry, Row, Scope, Search, Submission, Value,
};

/// The fixture, with a write that reports the row it locked: the title
/// another request wrote after the admin's read.
struct Moved(Fixture);

#[async_trait]
impl AdminData for Moved {
    fn registry(&self) -> Vec<RegistryEntry> {
        self.0.registry()
    }
    async fn count(
        &self,
        model: &ModelDescriptor,
        search: Search<'_>,
        scope: Scope<'_>,
    ) -> Result<usize, DataError> {
        self.0.count(model, search, scope).await
    }
    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        self.0.list(request).await
    }
    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<Option<Row>, DataError> {
        self.0.fetch(model, key, scope).await
    }
    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        self.0.insert(model, values).await
    }
    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        self.0.update(model, key, values, scope).await
    }
    async fn update_locked(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: Scope<'_>,
    ) -> Result<Locked, DataError> {
        let mut before = self.0.fetch(model, key, scope).await?;
        if let Some(row) = &mut before {
            row.set("title", Value::Text("Written meanwhile".into()));
        }
        let rows = self.0.update(model, key, values, scope).await?;
        Ok(Locked::new(rows, before))
    }
    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        self.0.delete(model, key, scope).await
    }
    async fn begin(&self) -> Result<Box<dyn AdminTransaction + '_>, DataError> {
        self.0.begin().await
    }
}

#[test]
fn an_edit_journals_the_row_the_write_locked() {
    let heard: Arc<Mutex<Vec<(String, Option<String>, Option<String>)>>> = Arc::default();
    let log = Arc::clone(&heard);
    let admin = Admin::new(
        Arc::new(Moved(Fixture::blog())),
        AdminConfig::new()
            .with_policy(AllowAllUnsafeForDevelopment)
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development()
            .journal(move |w: &AdminWrite<'_>| {
                let mut log = log.lock().unwrap();
                for f in w.fields {
                    log.push((f.name.clone(), f.before.clone(), f.after.clone()));
                }
            }),
    );
    let form = Submission::from_urlencoded(
        "title=Compilers%2C+revised&body=A0+and+after.&author_id=2&views=12&published_at=",
    );
    let answer =
        block_on(admin.dispatch(Principal::anonymous(), "POST", "/admin/post/2", "", &form))
            .unwrap();
    assert_eq!(answer.status(), 303);
    let heard = heard.lock().unwrap().clone();
    assert!(
        heard.contains(&(
            "title".into(),
            Some("Written meanwhile".into()),
            Some("Compilers, revised".into())
        )),
        "{heard:?}"
    );
}
