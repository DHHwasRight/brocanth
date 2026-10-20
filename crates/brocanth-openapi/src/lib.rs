//! An OpenAPI 3.1 document, derived from a brocanth route table.
//!
//! # The argument
//!
//! The route table already knows every path, every verb and every path
//! parameter, and it knows them at compile time, because in brocanth the
//! filesystem *is* the route table. A sibling crate, `brocanth-schema`,
//! describes request and response bodies. Between the two, **an OpenAPI
//! document is derivable rather than written** — and that is the whole
//! argument, because a hand-written document drifts from the code within one
//! sprint and nobody notices until a client breaks.
//!
//! So this crate generates the document, and then — the part that matters —
//! [`check_file`] fails the build when the committed document and the
//! generated one disagree. "The spec is out of date" stops being a discovery
//! made during an incident and becomes a red build on the pull request that
//! caused it.
//!
//! # A plugin, like everything else
//!
//! §8 of the plan: nothing ships unless it is asked for. `brocanth` does not
//! depend on this crate and never will; an application that does not name
//! [`OpenApiPlugin`] in `main` links none of it and carries no document in
//! its binary. This crate's own dependencies are `brocanth`, `http` and
//! `thiserror` — there is no serde here and no YAML crate, because the
//! document is built as an ordered value ([`Json`]) and written by hand.
//!
//! # Five minutes
//!
//! ```
//! use brocanth::routing::{Router, Verb};
//! use brocanth_openapi::{Schema, SpecBuilder};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let mut routes = Router::builder();
//! routes.route("/posts", Verb::Get, "index")?;
//! routes.route("/posts", Verb::Post, "create")?;
//! routes.route("/posts/{id}", Verb::Get, "show")?;
//! let router = routes.build()?;
//!
//! let document = SpecBuilder::new("Blog", "1.0.0")
//!     .component(
//!         "Post",
//!         Schema::object()
//!             .required("id", Schema::integer())
//!             .required("title", Schema::string())
//!             .property("body", Schema::string().nullable()),
//!     )
//!     .routes_from_router(&router)
//!     .describe("/posts", Verb::Get, |op| {
//!         op.summary("List posts")
//!             .json_response(200, "Every post", Schema::array(Schema::reference("Post")))
//!     })
//!     // `/posts/{id}` was never annotated, and still produces a valid
//!     // operation: an operationId, its path parameter, a tag and a 200.
//!     .build()?;
//!
//! assert_eq!(document.operation_count(), 3);
//! let show = document.path("/posts/{id}").unwrap().operation(Verb::Get).unwrap();
//! assert_eq!(show.operation_id, "get_posts_by_id");
//! assert_eq!(show.parameters.len(), 1);
//! # Ok(())
//! # }
//! ```
//!
//! # What this crate deliberately does not build
//!
//! **No client generators.** `openapi-generator` emits clients for fifty
//! languages, `oapi-codegen` and `progenitor` do Go and Rust better than a
//! side project would, and every one of them consumes exactly the artefact
//! this crate produces. Writing a fifty-first would mean competing with
//! tools that have years of per-language corner cases in them, and winning
//! nothing: a correct document is already the integration point.
//!
//! **No interactive try-it console.** Swagger UI's "Execute" button needs a
//! live server to send the request to, usually through a proxy that handles
//! CORS and injects credentials. A static document has nowhere to put one,
//! and an application that mounts a console has mounted a request-forwarding
//! endpoint — which is a security surface, not a documentation feature. Serve
//! the document; point Swagger UI, Redoc, Scalar or Stoplight at it. Those
//! are static sites and the choice stays the application's.
//!
//! **No runtime schema inference.** A body's shape comes from a type, through
//! [`SchemaSource`], at compile time. Watching traffic to guess a schema
//! produces a document that is right about last Tuesday.
//!
//! Emitting a valid document is the whole job.
//!
//! # The corners that are quiet failures
//!
//! Each of these is wrong in a way no tool reports at the point the mistake
//! is made, so each has a test:
//!
//! * **A path parameter in the URL and not in `parameters`** makes the
//!   document invalid. Parameters are derived from the route declaration, so
//!   there is nothing to forget, and [`Document::validate`] checks it anyway.
//! * **`operationId` must be unique across the whole document.** A generator
//!   resolves a collision by dropping one of the two. [`SpecBuilder::build`]
//!   refuses instead.
//! * **`required` is a list of names on the object**, not a flag on each
//!   property. [`Schema::required`] is the only way to add one, so the two
//!   cannot disagree.
//! * **Nullable in 3.1 is `type: [T, "null"]`**, not 3.0's `nullable: true`,
//!   which 3.1 ignores silently.
//! * **`$ref` siblings are legal in 3.1 and ignored by many tools.** This
//!   crate never emits one: a described reference is wrapped in a
//!   single-element `allOf`, and a nullable reference in an `anyOf`.
//! * **OpenAPI compares paths with their parameter names removed**, so
//!   `/users/{id}` and `/users/{userId}` are one path. Emitting both is
//!   refused rather than left for a parser to resolve by overwriting.
//!
//! [`check_file`]: drift::check_file

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod adapter;

/// The document's tab in `brocanth-admin`.
///
/// Behind an off-by-default feature, so an application that publishes a
/// document and has no backoffice links neither the admin nor a renderer.
#[cfg(feature = "admin")]
pub mod admin;
pub mod document;
pub mod drift;
pub mod json;
pub mod plugin;
pub mod schema;
pub mod spec;

/// What `brocanth create` writes when somebody asks for a document.
///
/// Behind an off-by-default feature, so an application that publishes a
/// document never links the code that writes one into a new project.
#[cfg(feature = "scaffold")]
pub mod scaffold;

/// The panel's screens, compiled in by rsx at macro expansion: nothing is
/// read from disk at run time.
///
/// At the crate root and not inside [`admin`], because a template's own
/// `use` line names this path. Private, like the admin's own template
/// module, which is also what keeps `missing_docs` off generated items.
#[cfg(feature = "admin")]
mod admin_templates {
    rsx::load_dir!("src/admin_templates");
}

pub use crate::adapter::{schema_for, SchemaNode};
pub use crate::document::{
    ApiKeyIn, ComponentError, Components, Contact, Document, ExternalDocs, Header, Info, License,
    MediaType, Operation, ParamIn, Parameter, PathItem, RequestBody, Response, SecurityRequirement,
    SecurityScheme, Server, Tag, Validation, OPENAPI_VERSION,
};
pub use crate::drift::{
    check, check_file, render, write, write_as, Change, Difference, Drift, DriftError, Format,
};
pub use crate::json::{Json, JsonError};
pub use crate::plugin::{document_of, OpenApiConfig, OpenApiPlugin, SharedDocument};
pub use crate::schema::{Additional, NamedSchema, Schema, SchemaKind, SchemaSource, SchemaType};
pub use crate::spec::{
    check_path_parameters, openapi_path, operation_id_for, DescribeOperation, HeadPolicy,
    OperationIdStyle, OperationSpec, SpecBuilder, SpecError,
};
