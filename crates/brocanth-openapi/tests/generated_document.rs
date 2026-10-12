//! The properties a generated document has to have, each one a failure that
//! no tool reports at the point the mistake is made.

use std::collections::HashSet;

use brocanth::routing::{Router, RouterBuilder, Verb};
use brocanth_openapi::{
    check, check_file, write, Document, Format, HeadPolicy, Json, Schema, SpecBuilder, SpecError,
};

// ---------------------------------------------------------------------------
// A small application: a blog with posts, comments and a docs catch-all.
// ---------------------------------------------------------------------------

/// The route table, exactly as the application's own would be built.
fn blog_router() -> Router<&'static str> {
    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/", Verb::Get, "home").unwrap();
    routes.route("/posts", Verb::Get, "posts_index").unwrap();
    routes.route("/posts", Verb::Post, "posts_create").unwrap();
    routes.route("/posts/{id}", Verb::Get, "posts_show").unwrap();
    routes
        .route("/posts/{id}", Verb::Patch, "posts_update")
        .unwrap();
    routes
        .route("/posts/{id}", Verb::Delete, "posts_destroy")
        .unwrap();
    routes
        .route("/posts/{id}/comments", Verb::Get, "comments_index")
        .unwrap();
    routes
        .route("/posts/{id}/comments", Verb::Post, "comments_create")
        .unwrap();
    routes.route("/docs/{*rest}", Verb::Get, "docs").unwrap();
    routes.build().unwrap()
}

fn post_schema() -> Schema {
    Schema::object()
        .required("id", Schema::integer())
        .required("title", Schema::string().max_length(120))
        .required("body", Schema::string().nullable())
        .property(
            "published_at",
            Schema::string().format("date-time").nullable(),
        )
}

/// What a create form sends: the model without the fields the server owns.
fn new_post_schema() -> Schema {
    Schema::object()
        .required("title", Schema::string().max_length(120))
        .property("body", Schema::string())
}

fn comment_schema() -> Schema {
    Schema::object()
        .required("id", Schema::integer())
        .required("post", Schema::reference("Post"))
        .required("body", Schema::string())
}

fn blog_document() -> Document {
    SpecBuilder::new("Blog", "1.0.0")
        .description("A blog with posts and comments.")
        .component("Post", post_schema())
        .component("NewPost", new_post_schema())
        .component("Comment", comment_schema())
        .routes_from_router(&blog_router())
        .describe("/posts", Verb::Get, |op| {
            op.summary("List posts")
                .query_param("page", Schema::integer().minimum(1.0))
                .json_response(200, "A page of posts", Schema::array(Schema::reference("Post")))
        })
        .describe("/posts", Verb::Post, |op| {
            op.summary("Create a post")
                .form_body(Schema::reference("NewPost"))
                .json_response(201, "The post that was created", Schema::reference("Post"))
                .status(422, "The form did not validate")
        })
        .describe("/posts/{id}", Verb::Get, |op| {
            op.summary("Show one post")
                .path_param("id", Schema::integer())
                .json_response(200, "The post", Schema::reference("Post"))
                .status(404, "No post has that id")
        })
        .describe("/posts/{id}/comments", Verb::Get, |op| {
            op.summary("List a post's comments").json_response(
                200,
                "Every comment on the post",
                Schema::array(Schema::reference("Comment")),
            )
        })
        .build()
        .expect("the blog document derives")
}

// ---------------------------------------------------------------------------
// Validity
// ---------------------------------------------------------------------------

#[test]
fn the_document_is_structurally_valid() {
    blog_document().validate().unwrap();
}

#[test]
fn the_document_has_the_fields_a_reader_expects() {
    let json = blog_document().to_json();
    assert_eq!(
        json.get("openapi").and_then(Json::as_str),
        Some("3.1.0"),
        "3.1 is what aligns with JSON Schema 2020-12"
    );
    for key in ["openapi", "info", "servers", "paths", "components", "tags"] {
        assert!(json.get(key).is_some(), "the document has no `{key}`");
    }
    let info = json.get("info").unwrap();
    assert!(info.get("title").is_some());
    assert!(info.get("version").is_some());
    // `servers` defaults to one relative entry rather than being left out.
    assert_eq!(json.get("servers").unwrap().as_array().unwrap().len(), 1);
}

#[test]
fn no_schema_uses_the_three_zero_spelling_of_nullable() {
    let text = blog_document().to_json_string();
    assert!(
        !text.contains("\"nullable\""),
        "3.1 ignores `nullable`; a nullable value is `type: [T, \"null\"]`"
    );
    assert!(text.contains("\"null\""), "something here is nullable");
}

#[test]
fn a_ref_never_carries_a_sibling() {
    // Legal in 3.1, ignored by many tools. Walk every object: one that has a
    // `$ref` must have nothing else.
    fn walk(value: &Json, at: &str) {
        match value {
            Json::Object(entries) => {
                if entries.iter().any(|(k, _)| k == "$ref") {
                    assert_eq!(
                        entries.len(),
                        1,
                        "{at} has a `$ref` with siblings: {:?}",
                        entries.iter().map(|(k, _)| k).collect::<Vec<_>>()
                    );
                }
                for (key, child) in entries {
                    walk(child, &format!("{at}/{key}"));
                }
            }
            Json::Array(items) => {
                for (index, child) in items.iter().enumerate() {
                    walk(child, &format!("{at}/{index}"));
                }
            }
            _ => {}
        }
    }
    walk(&blog_document().to_json(), "");
}

// ---------------------------------------------------------------------------
// Routes
// ---------------------------------------------------------------------------

#[test]
fn every_route_appears_exactly_once() {
    let router = blog_router();
    let document = blog_document();

    let mut expected: Vec<(String, Verb)> = Vec::new();
    for (template, allow) in router.routes() {
        for verb in allow.iter() {
            // HEAD is derived from GET by the router; the default policy
            // leaves the derived one out of the document.
            if verb == Verb::Head && allow.contains(Verb::Get) {
                continue;
            }
            expected.push((
                brocanth_openapi::openapi_path(template.pattern()),
                verb,
            ));
        }
    }

    let mut found: Vec<(String, Verb)> = document
        .operations()
        .map(|(path, verb, _)| (path.to_owned(), verb))
        .collect();

    assert_eq!(
        found.len(),
        expected.len(),
        "the document describes a different number of operations than the router answers"
    );
    let unique: HashSet<(String, Verb)> = found.iter().cloned().collect();
    assert_eq!(unique.len(), found.len(), "an operation appears twice");

    found.sort();
    let mut expected_sorted = expected.clone();
    expected_sorted.sort();
    assert_eq!(found, expected_sorted);
}

#[test]
fn an_undescribed_route_still_produces_a_valid_operation() {
    let document = blog_document();
    // `/posts/{id}` DELETE was never annotated.
    let operation = document
        .path("/posts/{id}")
        .unwrap()
        .operation(Verb::Delete)
        .unwrap();
    assert_eq!(operation.operation_id, "delete_posts_by_id");
    assert_eq!(operation.tags, vec!["posts".to_string()]);
    assert_eq!(operation.parameters.len(), 1);
    assert_eq!(operation.responses.len(), 1);
    assert_eq!(operation.responses[0].0, "200");
    assert!(!operation.responses[0].1.description.is_empty());
}

#[test]
fn an_implicit_head_is_left_out_unless_it_is_asked_for() {
    let document = blog_document();
    assert!(document.path("/posts").unwrap().operation(Verb::Head).is_none());

    let with_head = SpecBuilder::new("Blog", "1.0.0")
        .head_policy(HeadPolicy::Include)
        .routes_from_router(&blog_router())
        .build()
        .unwrap();
    assert!(with_head
        .path("/posts")
        .unwrap()
        .operation(Verb::Head)
        .is_some());
}

#[test]
fn a_catch_all_becomes_an_ordinary_path_parameter() {
    let document = blog_document();
    let item = document
        .path("/docs/{rest}")
        .expect("the catch-all loses its star");
    let operation = item.operation(Verb::Get).unwrap();
    let parameter = &operation.parameters[0];
    assert_eq!(parameter.name, "rest");
    assert!(parameter.required);
    assert!(
        parameter
            .description
            .as_deref()
            .unwrap_or_default()
            .contains('/'),
        "a catch-all may contain a slash and the document should say so"
    );
}

// ---------------------------------------------------------------------------
// Path parameters
// ---------------------------------------------------------------------------

#[test]
fn every_path_parameter_appears_in_parameters() {
    let document = blog_document();
    brocanth_openapi::check_path_parameters(&document).unwrap();

    // And the same check from the outside, against the emitted JSON, since
    // that is what a consumer reads.
    let json = document.to_json();
    let paths = json.get("paths").unwrap().as_object().unwrap();
    for (path, item) in paths {
        let names: Vec<&str> = path
            .split('{')
            .skip(1)
            .filter_map(|part| part.split('}').next())
            .collect();
        for (verb, operation) in item.as_object().unwrap() {
            let declared: Vec<&str> = operation
                .get("parameters")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .filter(|p| p.get("in").and_then(Json::as_str) == Some("path"))
                .filter_map(|p| p.get("name").and_then(Json::as_str))
                .collect();
            for name in &names {
                assert!(
                    declared.contains(name),
                    "{path} {verb}: `{{{name}}}` is in the path and not in `parameters`"
                );
            }
        }
    }
}

#[test]
fn a_path_parameter_keeps_the_type_the_annotation_gave_it() {
    let document = blog_document();
    let show = document
        .path("/posts/{id}")
        .unwrap()
        .operation(Verb::Get)
        .unwrap();
    let json = show.parameters[0].schema.to_json();
    assert_eq!(json.get("type"), Some(&Json::Str("integer".into())));
}

#[test]
fn describing_a_path_parameter_that_does_not_exist_is_refused() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts/{id}", Verb::Get, "show").unwrap();
    let router = routes.build().unwrap();

    let error = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .describe("/posts/{id}", Verb::Get, |op| {
            op.path_param("ids", Schema::integer())
        })
        .build()
        .unwrap_err();

    let SpecError::UnknownPathParameter {
        name, suggestion, ..
    } = error
    else {
        panic!("expected an unknown path parameter");
    };
    assert_eq!(name, "ids");
    assert_eq!(suggestion.as_deref(), Some("id"));
}

// ---------------------------------------------------------------------------
// operationId
// ---------------------------------------------------------------------------

#[test]
fn operation_ids_are_unique_across_the_document() {
    let document = blog_document();
    let mut seen = HashSet::new();
    for (path, verb, operation) in document.operations() {
        assert!(
            seen.insert(operation.operation_id.clone()),
            "{path} {verb}: the operationId `{}` is used twice",
            operation.operation_id
        );
    }
}

#[test]
fn a_duplicate_operation_id_is_refused() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    routes.route("/archive", Verb::Get, "archive").unwrap();
    let router = routes.build().unwrap();

    let error = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .describe("/posts", Verb::Get, |op| op.operation_id("list"))
        .describe("/archive", Verb::Get, |op| op.operation_id("list"))
        .build()
        .unwrap_err();

    let SpecError::DuplicateOperationId { id, .. } = error else {
        panic!("expected a duplicate operationId, got {error}");
    };
    assert_eq!(id, "list");
}

#[test]
fn two_paths_that_differ_only_by_parameter_name_are_refused() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts/{id}", Verb::Get, "show").unwrap();
    let router = routes.build().unwrap();

    let error = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .route("/posts/{slug}", Verb::Get)
        .build()
        .unwrap_err();

    assert!(
        matches!(error, SpecError::AmbiguousPaths { .. }),
        "got {error}"
    );
}

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

#[test]
fn a_shared_type_is_defined_once_and_referenced_everywhere() {
    let document = blog_document();
    let text = document.to_json_string();

    // One definition.
    assert_eq!(
        document.components.schema_names().collect::<Vec<_>>(),
        vec!["Comment", "NewPost", "Post"]
    );
    // `published_at` belongs to `Post` alone, so it appears once if and only
    // if `Post` is spelled out once.
    let definitions = text.matches("\"published_at\"").count();
    assert_eq!(definitions, 1, "`Post` is spelled out more than once");

    // Referenced from several operations, and from another component.
    let references = text.matches("#/components/schemas/Post").count();
    assert!(
        references >= 4,
        "`Post` is used by four places and referenced {references} times"
    );
}

#[test]
fn registering_one_name_for_two_shapes_is_refused() {
    let error = SpecBuilder::new("Blog", "1.0.0")
        .component("Post", Schema::object().required("id", Schema::integer()))
        .component("Post", Schema::string())
        .build()
        .unwrap_err();
    assert!(matches!(error, SpecError::Component(_)), "got {error}");
}

#[test]
fn a_reference_with_no_definition_is_refused() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    let router = routes.build().unwrap();

    let error = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .describe("/posts", Verb::Get, |op| {
            op.json_response(200, "posts", Schema::reference("Ghost"))
        })
        .build()
        .unwrap_err();
    assert!(matches!(error, SpecError::Invalid(_)), "got {error}");
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

#[test]
fn describing_a_route_that_is_not_registered_is_refused() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    let router = routes.build().unwrap();

    let error = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .describe("/post", Verb::Get, |op| op.summary("typo"))
        .build()
        .unwrap_err();

    let SpecError::UnknownRoute { suggestion, .. } = error else {
        panic!("expected an unknown route");
    };
    assert_eq!(suggestion.as_deref(), Some("/posts"));
}

#[test]
fn a_hidden_route_is_served_but_not_described() {
    let mut routes: RouterBuilder<&str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    routes.route("/openapi.json", Verb::Get, "spec").unwrap();
    let router = routes.build().unwrap();

    let document = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .hide("/openapi.json", Verb::Get)
        .build()
        .unwrap();

    assert!(document.path("/openapi.json").is_none());
    assert_eq!(document.operation_count(), 1);
}

#[test]
fn required_is_a_list_of_names_in_the_emitted_json() {
    let text = blog_document().to_json_string();
    assert!(text.contains("\"required\": [\n"), "`required` is a list");
    // And a property never carries a boolean `required` of its own.
    let json = blog_document().to_json();
    let post = json
        .get("components")
        .unwrap()
        .get("schemas")
        .unwrap()
        .get("Post")
        .unwrap();
    for (_, property) in post.get("properties").unwrap().as_object().unwrap() {
        assert!(property.get("required").is_none());
    }
}

// ---------------------------------------------------------------------------
// Drift
// ---------------------------------------------------------------------------

#[test]
fn drift_passes_on_an_unchanged_document() {
    let document = blog_document();
    let committed = document.to_json_string();
    check(&document, &committed, Format::Json).unwrap();
}

#[test]
fn drift_fires_when_a_route_is_added() {
    let committed = blog_document().to_json_string();

    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    routes.route("/authors", Verb::Get, "authors").unwrap();
    let router = routes.build().unwrap();
    let changed = SpecBuilder::new("Blog", "1.0.0")
        .description("A blog with posts and comments.")
        .routes_from_router(&router)
        .build()
        .unwrap();

    let error = check(&changed, &committed, Format::Json).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("out of date"), "{message}");
    assert!(
        message.contains("authors") || message.contains("~1authors"),
        "the report should name the route that moved: {message}"
    );
}

#[test]
fn drift_fires_when_a_verb_is_removed() {
    let committed = blog_document().to_json_string();

    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    let router = routes.build().unwrap();
    let changed = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .build()
        .unwrap();

    let error = check(&changed, &committed, Format::Json).unwrap_err();
    assert!(error.to_string().contains("removed"), "{error}");
}

#[test]
fn drift_ignores_formatting_and_key_order() {
    let document = blog_document();
    let committed = document.to_json_string();
    let reparsed = brocanth_openapi::json::parse(&committed).unwrap();

    // Reverse every object's key order, which changes the text completely and
    // the document not at all.
    fn reverse(value: &Json) -> Json {
        match value {
            Json::Object(entries) => Json::Object(
                entries
                    .iter()
                    .rev()
                    .map(|(k, v)| (k.clone(), reverse(v)))
                    .collect(),
            ),
            Json::Array(items) => Json::Array(items.iter().map(reverse).collect()),
            other => other.clone(),
        }
    }

    check(&document, &reverse(&reparsed).to_json(), Format::Json).unwrap();
}

#[test]
fn drift_against_a_file_on_disk() {
    let dir = std::env::temp_dir().join(format!(
        "brocanth-openapi-drift-{}",
        std::process::id()
    ));
    let path = dir.join("openapi.json");
    let document = blog_document();

    write(&document, &path).unwrap();
    check_file(&document, &path).unwrap();

    // A route arrives and nobody regenerates.
    let mut routes: RouterBuilder<&'static str> = Router::builder();
    routes.route("/posts", Verb::Get, "index").unwrap();
    let router = routes.build().unwrap();
    let changed = SpecBuilder::new("Blog", "1.0.0")
        .routes_from_router(&router)
        .build()
        .unwrap();
    let error = check_file(&changed, &path).unwrap_err();
    assert!(error.to_string().contains("openapi.json"), "{error}");

    // A missing file is a failure, never a reason to write one.
    let missing = dir.join("absent.json");
    assert!(check_file(&document, &missing).is_err());
    assert!(!missing.exists());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_yaml_is_the_same_document() {
    let document = blog_document();
    let yaml = document.to_yaml_string();
    assert!(yaml.starts_with("openapi: \"3.1.0\"\n"));
    assert!(yaml.contains("  \"/posts/{id}\":\n"));
    assert!(yaml.ends_with('\n'));
}
