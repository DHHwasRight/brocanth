//! The tab data contract over the admin's in-memory fixture: each call and
//! each answer survives JSON, the admin's end and the data side agree with
//! the source, the claim reaches the source, and a transaction borrows
//! nothing.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use brocanth_admin::data::{
    AdminData, AdminTransaction, DataError, FieldErrors, ListRequest, RequestCx,
    Scope as DataScope, Search as DataSearch, SharedData,
};
use brocanth_admin::discovery::RegistryEntry;
use brocanth_admin::model::{ModelDescriptor, PrimaryKey, Row, Value};
use brocanth_admin::policy::{AllowAllUnsafeForDevelopment, Level, Principal};
use brocanth_admin::query::{Direction, Sort};
use brocanth_admin::rt::block_on;
use brocanth_admin::{discover, Admin, AdminConfig, Discovery, Fixture, ListQuery};
use brocanth_backoffice::json::trip;
use brocanth_backoffice::{
    Answer, Call, Caller, Claim, Contracted, ErrorKind, InProcess, Json, Op, Read, Scope, Search,
    TabData, TabError, Write,
};

fn post(data: &dyn AdminData) -> ModelDescriptor {
    discover(data.registry(), &Discovery::new())
        .into_iter()
        .find(|m| m.key == "post")
        .expect("the fixture holds `post`")
}

fn everything() -> DataSearch<'static> {
    DataSearch {
        term: "",
        columns: &[],
    }
}

/// The fixture behind the contract, with JSON between the two ends.
fn contracted(fixture: Arc<Fixture>) -> SharedData {
    Contracted::new(Json::new(InProcess::new(fixture))).shared()
}

fn title(data: &dyn AdminData, id: i64) -> Option<String> {
    let model = post(data);
    block_on(data.fetch(&model, &PrimaryKey::single("id", id), DataScope::Unscoped))
        .expect("a fetch")
        .and_then(|row| row.get("title").map(ToString::to_string))
}

#[test]
fn every_call_and_answer_survives_json() {
    // A condition as the admin compiles one: SQL text and typed binds.
    let condition = Scope::Where(
        serde_json::from_value(serde_json::json!({
            "sql": "\"post\".\"author_id\" = $1",
            "binds": [{"Int": 7}],
            "dialect": "Postgres"
        }))
        .expect("a condition decodes"),
    );
    let caller = Caller {
        tab: "post".into(),
        level: Level::Edit,
        claim: Claim::staff("7", false, ["post.edit"]),
    };
    let calls = [
        Op::Read(Read::List {
            model: "post".into(),
            offset: 30,
            limit: 30,
            search: Search {
                term: "a\"b".into(),
                columns: vec!["title".into()],
            },
            sort: Some(Sort {
                column: "views".into(),
                direction: Direction::Descending,
            }),
            filter: Some(brocanth_backoffice::contract::Equals {
                column: "author_id".into(),
                value: Value::Int(7),
            }),
            scope: condition.clone(),
        }),
        Op::Read(Read::FetchMany {
            model: "post".into(),
            column: "id".into(),
            keys: vec!["1".into(), "2".into()],
            scope: Scope::Unscoped,
        }),
        Op::Write(Write::Update {
            model: "post".into(),
            key: PrimaryKey::from_parts([("id", "1")]),
            values: vec![
                ("title".into(), Value::Text("é, \"quoted\"".into())),
                ("views".into(), Value::Float(1.5)),
                ("published".into(), Value::Bool(true)),
                ("published_at".into(), Value::Null),
            ],
            scope: condition,
        }),
    ];
    for op in calls {
        let call = Call {
            caller: caller.clone(),
            op,
        };
        assert_eq!(trip(&call).expect("a call survives"), call);
    }
    let answers = [
        Answer::Count(2),
        Answer::Row(Some(Row::new().with("id", 1).with("title", "x"))),
        Answer::Key(PrimaryKey::single("id", 3)),
        Answer::Changed {
            rows: 1,
            before: Some(Row::new().with("id", 1)),
        },
        Answer::Done,
    ];
    for answer in answers {
        assert_eq!(trip(&answer).expect("an answer survives"), answer);
    }
    let invalid = TabError::from(DataError::Invalid(
        FieldErrors::new().with("email", "is taken"),
    ));
    let back = DataError::from(trip(&invalid).expect("an error survives"));
    let fields = back.field_errors().expect("still the field errors");
    assert_eq!(fields.on("email").collect::<Vec<_>>(), ["is taken"]);
}

#[test]
fn the_contract_answers_as_the_source_does() {
    let fixture = Arc::new(Fixture::blog());
    let data = contracted(Arc::clone(&fixture));
    let model = post(&*data);

    assert_eq!(
        block_on(data.count(&model, everything(), DataScope::Unscoped)).unwrap(),
        block_on(fixture.count(&model, everything(), DataScope::Unscoped)).unwrap(),
    );
    let sort = Sort {
        column: "views".into(),
        direction: Direction::Ascending,
    };
    let request = ListRequest {
        model: &model,
        offset: 0,
        limit: 10,
        search: everything(),
        sort: Some(&sort),
        filter: None,
        scope: DataScope::Unscoped,
    };
    assert_eq!(
        block_on(data.list(request)).unwrap(),
        block_on(fixture.list(request)).unwrap()
    );
    let keys = ["1".to_string(), "2".to_string(), "99".to_string()];
    let many = block_on(data.fetch_many(&model, "id", &keys, DataScope::Unscoped)).unwrap();
    assert_eq!(many.len(), 2, "a key that matches nothing is left out");

    let key = block_on(data.insert(
        &model,
        &[
            ("title".into(), "Through the contract".into()),
            ("body".into(), "".into()),
            ("author_id".into(), 1.into()),
            ("published".into(), false.into()),
            ("views".into(), 0.into()),
        ],
    ))
    .expect("an insert");
    let id: i64 = key.get("id").expect("an id").parse().expect("a number");
    assert_eq!(
        title(&*fixture, id).as_deref(),
        Some("Through the contract")
    );

    let locked = block_on(data.update_locked(
        &model,
        &key,
        &[("title".into(), "Renamed".into())],
        DataScope::Unscoped,
    ))
    .expect("an update");
    assert_eq!(locked.rows, 1);
    assert_eq!(title(&*fixture, id).as_deref(), Some("Renamed"));

    assert_eq!(
        block_on(data.delete(&model, &key, DataScope::Unscoped)).unwrap(),
        1
    );
    assert_eq!(title(&*fixture, id), None);
    assert_eq!(
        block_on(data.delete(&model, &key, DataScope::Unscoped)).unwrap(),
        0,
        "a missing row is zero rows, never success"
    );
}

#[test]
fn a_transaction_keeps_all_of_its_writes_or_none() {
    let fixture = Arc::new(Fixture::blog());
    let data = contracted(Arc::clone(&fixture));
    let model = post(&*data);
    let one = PrimaryKey::single("id", 1);
    let two = PrimaryKey::single("id", 2);
    let rename = |name: &str| vec![("title".to_string(), Value::from(name))];

    // Rolled back: nothing kept.
    let mut tx = block_on(data.begin()).expect("a transaction");
    assert_eq!(
        block_on(tx.update(&model, &one, &rename("Gone"), DataScope::Unscoped)).unwrap(),
        1
    );
    block_on(tx.rollback()).expect("a rollback");
    assert_eq!(
        title(&*fixture, 1).as_deref(),
        Some("On the Analytical Engine")
    );

    // Dropped: nothing kept either.
    let mut tx = block_on(data.begin()).expect("a transaction");
    block_on(tx.update(&model, &one, &rename("Dropped"), DataScope::Unscoped)).unwrap();
    drop(tx);
    assert_eq!(
        title(&*fixture, 1).as_deref(),
        Some("On the Analytical Engine")
    );

    // Committed: both kept.
    let mut tx = block_on(data.begin()).expect("a transaction");
    block_on(tx.update(&model, &one, &rename("Kept one"), DataScope::Unscoped)).unwrap();
    block_on(tx.update(&model, &two, &rename("Kept two"), DataScope::Unscoped)).unwrap();
    block_on(tx.commit()).expect("a commit");
    assert_eq!(title(&*fixture, 1).as_deref(), Some("Kept one"));
    assert_eq!(title(&*fixture, 2).as_deref(), Some("Kept two"));
}

/// A source that records the claim each request reached it with.
struct Seen {
    inner: Arc<Fixture>,
    seen: Arc<Mutex<Vec<Option<Claim>>>>,
}

#[async_trait]
impl AdminData for Seen {
    fn for_request(&self, cx: &RequestCx<'_>) -> Option<SharedData> {
        self.seen
            .lock()
            .unwrap()
            .push(cx.principal.downcast::<Claim>().cloned());
        None
    }
    fn registry(&self) -> Vec<RegistryEntry> {
        self.inner.registry()
    }
    async fn count(
        &self,
        model: &ModelDescriptor,
        search: DataSearch<'_>,
        scope: DataScope<'_>,
    ) -> Result<usize, DataError> {
        self.inner.count(model, search, scope).await
    }
    async fn list(&self, request: ListRequest<'_>) -> Result<Vec<Row>, DataError> {
        self.inner.list(request).await
    }
    async fn fetch(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: DataScope<'_>,
    ) -> Result<Option<Row>, DataError> {
        self.inner.fetch(model, key, scope).await
    }
    async fn insert(
        &self,
        model: &ModelDescriptor,
        values: &[(String, Value)],
    ) -> Result<PrimaryKey, DataError> {
        self.inner.insert(model, values).await
    }
    async fn update(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        values: &[(String, Value)],
        scope: DataScope<'_>,
    ) -> Result<u64, DataError> {
        self.inner.update(model, key, values, scope).await
    }
    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: DataScope<'_>,
    ) -> Result<u64, DataError> {
        self.inner.delete(model, key, scope).await
    }
    async fn begin(&self) -> Result<Box<dyn AdminTransaction + '_>, DataError> {
        self.inner.begin().await
    }
}

/// The application's own principal type, as the admin's policy sees it.
struct Staff {
    id: u32,
}

#[test]
fn the_claim_of_each_request_reaches_the_source() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let source = Arc::new(Seen {
        inner: Arc::new(Fixture::blog()),
        seen: Arc::clone(&seen),
    });
    let config = AdminConfig::new()
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment)
        .data_through(|data| {
            Contracted::new(Json::new(InProcess::new(data)))
                .claim(
                    |principal: &Principal<'_>| match principal.downcast::<Staff>() {
                        Some(staff) => Claim::staff(staff.id.to_string(), false, ["post.read"]),
                        None => Claim::anonymous(),
                    },
                )
                .shared()
        });
    let admin = Admin::try_new(source, config).expect("the admin builds");
    let ada = Staff { id: 7 };
    let page = block_on(admin.list(Principal::new(&ada), "post", ListQuery::new())).unwrap();
    assert_eq!(page.status(), 200);
    block_on(admin.list(Principal::anonymous(), "post", ListQuery::new())).unwrap();

    let seen = seen.lock().unwrap();
    assert!(
        seen.iter().any(|claim| claim
            .as_ref()
            .is_some_and(|c| c.subject.as_deref() == Some("7") && c.grants == ["post.read"])),
        "the source never saw Ada's claim: {seen:?}"
    );
    // Nobody is the anonymous principal on the data side, not a claim.
    assert!(seen.iter().any(Option::is_none), "{seen:?}");
}

#[test]
fn a_claim_holds_a_level_and_every_level_below_it() {
    let editor = Claim::staff("1", false, ["post.edit"]);
    assert!(editor.holds("post", Level::Read));
    assert!(editor.holds("post", Level::Edit));
    assert!(!editor.holds("post", Level::Owner));
    assert!(!editor.holds("user", Level::Read), "a grant is per tab");
    assert!(Claim::staff("2", true, Vec::<String>::new()).holds("user", Level::Owner));
    assert!(!Claim::anonymous().holds("post", Level::Read));
    assert!(Claim::from_principal(&Principal::anonymous()).is_anonymous());
}

#[test]
fn a_data_side_that_enforces_grants_refuses_a_write_the_claim_does_not_hold() {
    let fixture = Arc::new(Fixture::blog());
    let tabs = InProcess::new(Arc::clone(&fixture) as SharedData).enforcing_grants();
    let reader = Caller {
        tab: "post".into(),
        level: Level::Edit,
        claim: Claim::staff("3", false, ["post.read"]),
    };
    let write = Op::Write(Write::Delete {
        model: "post".into(),
        key: PrimaryKey::single("id", 1),
        scope: Scope::Unscoped,
    });
    let refused = block_on(tabs.call(Call {
        caller: reader,
        op: write.clone(),
    }))
    .expect_err("a reader cannot delete");
    assert_eq!(refused.kind, ErrorKind::Refused);
    assert!(refused.to_string().contains("post.edit"), "{refused}");
    assert!(refused.to_string().contains("help:"), "{refused}");
    assert!(title(&*fixture, 1).is_some(), "the row is still there");

    // A call that claims less than its operation needs is not a call.
    let understated = block_on(tabs.call(Call {
        caller: Caller {
            tab: "post".into(),
            level: Level::Read,
            claim: Claim::staff("4", true, Vec::<String>::new()),
        },
        op: write,
    }))
    .expect_err("a delete needs edit");
    assert_eq!(understated.kind, ErrorKind::Protocol);
}

#[test]
fn an_unknown_model_is_named_with_a_help_line() {
    let tabs = InProcess::new(Arc::new(Fixture::blog()));
    let error = block_on(tabs.call(Call {
        caller: Caller {
            tab: "nope".into(),
            level: Level::Read,
            claim: Claim::anonymous(),
        },
        op: Op::Read(Read::Count {
            model: "nope".into(),
            search: Search::default(),
            scope: Scope::Unscoped,
        }),
    }))
    .expect_err("no such model");
    assert!(matches!(error.kind, ErrorKind::UnknownModel { ref model } if model == "nope"));
    assert!(error.to_string().contains("help:"), "{error}");
}

/// A source whose own configuration is wrong.
struct Misconfigured(Fixture);

#[async_trait]
impl AdminData for Misconfigured {
    fn check(&self) -> Result<(), DataError> {
        Err(DataError::backend(
            "the identity names no role\nhelp: give the identity a role",
        ))
    }
    fn registry(&self) -> Vec<RegistryEntry> {
        self.0.registry()
    }
    async fn count(
        &self,
        model: &ModelDescriptor,
        search: DataSearch<'_>,
        scope: DataScope<'_>,
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
        scope: DataScope<'_>,
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
        scope: DataScope<'_>,
    ) -> Result<u64, DataError> {
        self.0.update(model, key, values, scope).await
    }
    async fn delete(
        &self,
        model: &ModelDescriptor,
        key: &PrimaryKey,
        scope: DataScope<'_>,
    ) -> Result<u64, DataError> {
        self.0.delete(model, key, scope).await
    }
    async fn begin(&self) -> Result<Box<dyn AdminTransaction + '_>, DataError> {
        self.0.begin().await
    }
}

#[test]
fn the_sources_own_check_fails_the_build_of_the_admin_through_the_contract() {
    let config = AdminConfig::new()
        .data_through(|data| Contracted::new(Json::new(InProcess::new(data))).shared());
    let Err(error) = Admin::try_new(Arc::new(Misconfigured(Fixture::blog())), config) else {
        panic!("a source that refuses its configuration stops the admin");
    };
    let text = error.to_string();
    assert!(text.contains("the identity names no role"), "{text}");
    assert!(text.contains("help: give the identity a role"), "{text}");
}
