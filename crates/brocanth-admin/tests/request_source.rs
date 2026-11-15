//! A data source can serve each request through a source of its own, such
//! as a connection opened as the person's database role.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::data::{AdminTransaction, RequestCx};
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::{
    Admin, AdminConfig, AdminData, DataError, Discovery, Fixture, ListRequest, ModelDescriptor,
    PrimaryKey, Principal, RegistryEntry, Row, Scope, Search, SharedData, Submission, Value,
};

/// Anonymous requests read `shared`; a signed-in one reads `theirs`.
struct PerPerson {
    shared: Fixture,
    theirs: SharedData,
}

#[async_trait]
impl AdminData for PerPerson {
    fn for_request(&self, cx: &RequestCx<'_>) -> Option<SharedData> {
        (!cx.principal.is_anonymous()).then(|| Arc::clone(&self.theirs))
    }
    fn registry(&self) -> Vec<RegistryEntry> {
        self.shared.registry()
    }
    async fn count(
        &self,
        model: &ModelDescriptor,
        search: Search<'_>,
        scope: Scope<'_>,
    ) -> Result<usize, DataError> {
        self.shared.count(model, search, scope).await
    }
    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        self.shared.list(request).await
    }
    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<Option<Row>, DataError> {
        self.shared.fetch(model, key, scope).await
    }
    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        self.shared.insert(model, values).await
    }
    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        self.shared.update(model, key, values, scope).await
    }
    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: Scope<'_>,
    ) -> Result<u64, DataError> {
        self.shared.delete(model, key, scope).await
    }
    async fn begin(&self) -> Result<Box<dyn AdminTransaction + '_>, DataError> {
        self.shared.begin().await
    }
}

#[test]
fn each_request_reads_through_the_source_its_principal_gets() {
    let theirs = Fixture::blog();
    let post = brocanth_admin::discover(theirs.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("the post model");
    block_on(theirs.update(
        &post,
        &PrimaryKey::single("id", 1),
        &[("title".into(), Value::Text("Only theirs".into()))],
        Scope::Unscoped,
    ))
    .expect("an update");
    let admin = Admin::new(
        Arc::new(PerPerson {
            shared: Fixture::blog(),
            theirs: Arc::new(theirs),
        }),
        AdminConfig::new()
            .with_policy(AllowAllUnsafeForDevelopment)
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .unscoped_rows_unsafe_for_development(),
    );
    let page = |who: Principal<'_>| {
        let form = Submission::new();
        block_on(admin.dispatch(who, "GET", "/admin/post/1", "", &form))
            .unwrap()
            .html()
            .expect("a page")
            .to_string()
    };
    struct Ada;
    let ada = Ada;
    assert!(page(Principal::new(&ada)).contains("Only theirs"));
    assert!(!page(Principal::anonymous()).contains("Only theirs"));
}
