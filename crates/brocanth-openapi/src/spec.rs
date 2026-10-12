//! Deriving the document from the route table.
//!
//! # The argument
//!
//! The route table already knows every path, every verb and every path
//! parameter, and it knows them at compile time because the filesystem is the
//! route table. A document built from it cannot describe a path that does not
//! exist, cannot miss a path that does, and cannot disagree with the server
//! about which verbs answer. A hand-written document can do all three, and
//! does, usually within a sprint and usually unnoticed until a client breaks.
//!
//! So the document is derived. [`SpecBuilder`] takes routes — from a
//! [`Router`](brocanth::routing::Router), or named one at a time — and
//! annotations, and produces a [`Document`]. The annotations are optional
//! everywhere: an operation nobody described still gets an `operationId`,
//! its path parameters, a default tag and a `200` response, because a
//! generator that only works when every handler is decorated produces a
//! half-empty document forever.
//!
//! # Two conversions worth knowing about
//!
//! **Catch-all parameters.** brocanth writes `/docs/{*rest}`; OpenAPI has no
//! catch-all syntax at all, so the path key becomes `/docs/{rest}` and the
//! parameter carries a description saying it may contain `/`. That is the
//! closest honest representation; pretending the slash is not there would be
//! worse.
//!
//! **Implicit HEAD.** The router answers HEAD wherever GET is registered, as
//! RFC 9110 requires, so [`Allow`](brocanth::routing::Allow) reports it.
//! Emitting an extra operation for every GET doubles the document and tells a
//! reader nothing they did not already know, so [`HeadPolicy::Skip`] is the
//! default. A route that registers HEAD *without* GET is a deliberate
//! declaration and is always emitted.

use std::collections::BTreeMap;

use brocanth::routing::{ParamKind, RouteTemplate, Router, TemplateError, Verb};
use brocanth::suggest::nearest_name;

use crate::document::{
    path_variables, ComponentError, Contact, Document, ExternalDocs, Info, License, Operation,
    ParamIn, Parameter, PathItem, RequestBody, Response, SecurityRequirement, SecurityScheme,
    Server, Tag,
};
use crate::json::Json;
use crate::schema::{NamedSchema, Schema, SchemaSource};

/// A document that cannot be derived.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum SpecError {
    /// A template does not parse.
    #[error("the route template `{template}` does not parse: {source}")]
    Template {
        /// The rejected template.
        template: String,
        /// Why it was rejected.
        #[source]
        source: TemplateError,
    },

    /// An annotation names a route that is not in the table.
    #[error(
        "`{verb} {template}` was described, but no such route is registered{}",
        crate::spec::did_you_mean(.suggestion)
    )]
    UnknownRoute {
        /// The verb that was described.
        verb: Verb,
        /// The template that was described.
        template: String,
        /// The nearest registered path, when one is close.
        suggestion: Option<String>,
    },

    /// The same route was added twice.
    #[error("`{verb} {template}` is registered twice; every route appears exactly once")]
    DuplicateRoute {
        /// The repeated verb.
        verb: Verb,
        /// The repeated template.
        template: String,
    },

    /// Two operations want one `operationId`.
    #[error(
        "`{first}` and `{second}` both produce the operationId `{id}`; it must be unique across \
         the document, because a client generator turns it into a method name"
    )]
    DuplicateOperationId {
        /// The contested identifier.
        id: String,
        /// The first operation to claim it.
        first: String,
        /// The second operation to claim it.
        second: String,
    },

    /// Two templates collapse onto one OpenAPI path key.
    #[error(
        "`{first}` and `{second}` are different routes but the same OpenAPI path; OpenAPI \
         compares paths with their parameter names removed, so the two cannot both appear"
    )]
    AmbiguousPaths {
        /// The first template.
        first: String,
        /// The second template.
        second: String,
    },

    /// An annotation refines a path parameter the template does not have.
    #[error(
        "`{verb} {template}` describes the path parameter `{name}`, which the template does not \
         declare{}",
        crate::spec::did_you_mean(.suggestion)
    )]
    UnknownPathParameter {
        /// The verb of the operation.
        verb: Verb,
        /// The template of the operation.
        template: String,
        /// The parameter that does not exist.
        name: String,
        /// The nearest declared parameter, when one is close.
        suggestion: Option<String>,
    },

    /// A component could not be registered.
    #[error(transparent)]
    Component(#[from] ComponentError),

    /// The derived document is not structurally valid.
    ///
    /// This should not happen, and when it does the bug is in this crate
    /// rather than in the application. The check runs anyway: a generator
    /// that emits an invalid document and says nothing is the failure the
    /// crate exists to prevent.
    #[error(transparent)]
    Invalid(#[from] crate::document::Validation),
}

#[doc(hidden)]
pub fn did_you_mean(suggestion: &Option<String>) -> String {
    match suggestion {
        Some(name) => format!("; did you mean `{name}`?"),
        None => String::new(),
    }
}

/// What to do with the HEAD the router derives from a GET.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeadPolicy {
    /// Leave implicit HEAD operations out. The default.
    #[default]
    Skip,
    /// Emit a HEAD operation wherever the router answers one.
    Include,
}

/// How an `operationId` is spelled when nobody gives one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperationIdStyle {
    /// `get_posts_by_id`. The default: it reads as Rust, and every generator
    /// that wants camelCase converts it without ambiguity.
    #[default]
    Snake,
    /// `getPostsById`.
    Camel,
}

// ---------------------------------------------------------------------------
// Annotations
// ---------------------------------------------------------------------------

/// Everything a handler can say about its operation, and nothing it must.
///
/// Every field has a default that produces a valid entry, so annotating is a
/// way to say more rather than a precondition for saying anything.
///
/// ```
/// use brocanth_openapi::{OperationSpec, Schema};
///
/// let spec = OperationSpec::new()
///     .summary("List posts")
///     .tag("posts")
///     .query_param("page", Schema::integer().minimum(1.0))
///     .json_response(200, "A page of posts", Schema::array(Schema::reference("Post")));
/// ```
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OperationSpec {
    operation_id: Option<String>,
    summary: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    deprecated: bool,
    hidden: bool,
    path_params: Vec<(String, Schema, Option<String>)>,
    parameters: Vec<Parameter>,
    request_body: Option<RequestBody>,
    responses: Vec<(String, Response)>,
    security: Option<Vec<SecurityRequirement>>,
    external_docs: Option<ExternalDocs>,
}

impl OperationSpec {
    /// An annotation that says nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Overrides the generated `operationId`.
    #[must_use]
    pub fn operation_id(mut self, id: impl Into<String>) -> Self {
        self.operation_id = Some(id.into());
        self
    }

    /// One line.
    #[must_use]
    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.summary = Some(text.into());
        self
    }

    /// More than one line. CommonMark.
    #[must_use]
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Adds a tag. Replaces the tag derived from the path.
    #[must_use]
    pub fn tag(mut self, name: impl Into<String>) -> Self {
        self.tags.push(name.into());
        self
    }

    /// Marks the operation deprecated.
    #[must_use]
    pub fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }

    /// Leaves the operation out of the document.
    ///
    /// For a route that exists and is not part of the API: the document's own
    /// `/openapi.json`, a health check, an asset handler.
    #[must_use]
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    /// Gives a path parameter a type better than `string`.
    ///
    /// The name must be one the template declares; a typo is a build error
    /// rather than a parameter nobody notices is missing.
    #[must_use]
    pub fn path_param(mut self, name: impl Into<String>, schema: Schema) -> Self {
        self.path_params.push((name.into(), schema, None));
        self
    }

    /// A path parameter with a type and a description.
    #[must_use]
    pub fn path_param_described(
        mut self,
        name: impl Into<String>,
        schema: Schema,
        description: impl Into<String>,
    ) -> Self {
        self.path_params
            .push((name.into(), schema, Some(description.into())));
        self
    }

    /// An optional query parameter.
    #[must_use]
    pub fn query_param(self, name: impl Into<String>, schema: Schema) -> Self {
        self.parameter(Parameter::query(name, schema))
    }

    /// A required query parameter.
    #[must_use]
    pub fn required_query_param(self, name: impl Into<String>, schema: Schema) -> Self {
        self.parameter(Parameter::query(name, schema).required())
    }

    /// A request header.
    #[must_use]
    pub fn header_param(self, name: impl Into<String>, schema: Schema) -> Self {
        self.parameter(Parameter::new(name, ParamIn::Header, schema))
    }

    /// A cookie.
    #[must_use]
    pub fn cookie_param(self, name: impl Into<String>, schema: Schema) -> Self {
        self.parameter(Parameter::new(name, ParamIn::Cookie, schema))
    }

    /// Any parameter, built by hand.
    #[must_use]
    pub fn parameter(mut self, parameter: Parameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    /// The request body.
    #[must_use]
    pub fn body(mut self, body: RequestBody) -> Self {
        self.request_body = Some(body);
        self
    }

    /// A JSON request body.
    #[must_use]
    pub fn json_body(self, schema: Schema) -> Self {
        self.body(RequestBody::json(schema))
    }

    /// A form request body, which is what an HTML page posts.
    #[must_use]
    pub fn form_body(self, schema: Schema) -> Self {
        self.body(RequestBody::form(schema))
    }

    /// A response, built by hand. The first call replaces the default `200`.
    #[must_use]
    pub fn response(mut self, status: u16, response: Response) -> Self {
        self.responses.push((status.to_string(), response));
        self
    }

    /// The `default` response, for everything not listed.
    #[must_use]
    pub fn default_response(mut self, response: Response) -> Self {
        self.responses.push(("default".to_owned(), response));
        self
    }

    /// A response with a description and no body.
    #[must_use]
    pub fn status(self, status: u16, description: impl Into<String>) -> Self {
        self.response(status, Response::new(description))
    }

    /// A response with a JSON body.
    #[must_use]
    pub fn json_response(
        self,
        status: u16,
        description: impl Into<String>,
        schema: Schema,
    ) -> Self {
        self.response(status, Response::json(description, schema))
    }

    /// A response with an HTML body.
    #[must_use]
    pub fn html_response(self, status: u16, description: impl Into<String>) -> Self {
        self.response(status, Response::html(description))
    }

    /// What this operation requires, overriding the document default.
    #[must_use]
    pub fn security(mut self, requirement: SecurityRequirement) -> Self {
        self.security.get_or_insert_with(Vec::new).push(requirement);
        self
    }

    /// Marks the operation public in an otherwise protected API.
    ///
    /// Emits `security: []`, which is the only way to say "no authentication
    /// here" once the document has a top-level `security`.
    #[must_use]
    pub fn public(mut self) -> Self {
        self.security = Some(Vec::new());
        self
    }

    /// A link out.
    #[must_use]
    pub fn external_docs(mut self, url: impl Into<String>, description: Option<String>) -> Self {
        self.external_docs = Some(ExternalDocs {
            url: url.into(),
            description,
        });
        self
    }
}

/// A type whose operation annotation lives with the handler rather than with
/// the builder.
///
/// The builder is the whole annotation mechanism; this trait is the hook that
/// lets a handler keep its own description next to its own code, which is
/// where it stays correct. A unit struct per handler implements it, and the
/// application wires it in with [`SpecBuilder::describe_with`].
///
/// ```
/// use brocanth_openapi::{DescribeOperation, OperationSpec};
///
/// struct ListPosts;
///
/// impl DescribeOperation for ListPosts {
///     fn describe() -> OperationSpec {
///         OperationSpec::new().summary("List posts").tag("posts")
///     }
/// }
/// ```
pub trait DescribeOperation {
    /// What this handler wants said about it.
    fn describe() -> OperationSpec;
}

// ---------------------------------------------------------------------------
// The builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RouteEntry {
    template: RouteTemplate,
    verb: Verb,
}

/// Builds a [`Document`] from a route table and a set of annotations.
#[derive(Debug)]
pub struct SpecBuilder {
    document: Document,
    routes: Vec<RouteEntry>,
    annotations: BTreeMap<(String, Verb), OperationSpec>,
    head_policy: HeadPolicy,
    id_style: OperationIdStyle,
    errors: Vec<SpecError>,
}

impl SpecBuilder {
    /// A builder for an API with a title and a version.
    ///
    /// Both are required by the specification. The document starts with one
    /// server, `/`, because a relative server is the only honest default for
    /// an application that does not know its own public URL.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            document: Document::new(title, version),
            routes: Vec::new(),
            annotations: BTreeMap::new(),
            head_policy: HeadPolicy::default(),
            id_style: OperationIdStyle::default(),
            errors: Vec::new(),
        }
    }

    /// Replaces the whole `info` object.
    #[must_use]
    pub fn info(mut self, info: Info) -> Self {
        self.document.info = info;
        self
    }

    /// The API description. CommonMark.
    #[must_use]
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.document.info.description = Some(text.into());
        self
    }

    /// The API summary, one line. Added in 3.1.
    #[must_use]
    pub fn summary(mut self, text: impl Into<String>) -> Self {
        self.document.info.summary = Some(text.into());
        self
    }

    /// Who to contact about the API.
    #[must_use]
    pub fn contact(mut self, contact: Contact) -> Self {
        self.document.info.contact = Some(contact);
        self
    }

    /// The licence the API is published under.
    ///
    /// 3.1 added `identifier` for an SPDX expression and made it mutually
    /// exclusive with `url`; [`Document::validate`] enforces that.
    #[must_use]
    pub fn license(mut self, license: License) -> Self {
        self.document.info.license = Some(license);
        self
    }

    /// A terms-of-service URL.
    #[must_use]
    pub fn terms_of_service(mut self, url: impl Into<String>) -> Self {
        self.document.info.terms_of_service = Some(url.into());
        self
    }

    /// A link out, for the document as a whole.
    #[must_use]
    pub fn external_docs(mut self, url: impl Into<String>, description: Option<String>) -> Self {
        self.document.external_docs = Some(ExternalDocs {
            url: url.into(),
            description,
        });
        self
    }

    /// Adds a server. The first call replaces the default `/`.
    #[must_use]
    pub fn server(mut self, server: Server) -> Self {
        if self.document.servers.len() == 1 && self.document.servers[0].url == "/" {
            self.document.servers.clear();
        }
        self.document.servers.push(server);
        self
    }

    /// Describes a tag. Operations may use tags that are not declared here;
    /// those end up in `tags` with no description.
    #[must_use]
    pub fn tag(mut self, tag: Tag) -> Self {
        self.document.tags.push(tag);
        self
    }

    /// Registers a security scheme.
    #[must_use]
    pub fn security_scheme(mut self, name: impl Into<String>, scheme: SecurityScheme) -> Self {
        if let Err(error) = self.document.components.security_scheme(name, scheme) {
            self.errors.push(error.into());
        }
        self
    }

    /// Requires a scheme of every operation that does not say otherwise.
    #[must_use]
    pub fn require(mut self, scheme: impl Into<String>) -> Self {
        self.document
            .security
            .push(SecurityRequirement::new(scheme));
        self
    }

    /// Registers a reusable schema under `components/schemas`.
    ///
    /// Refer to it with [`Schema::reference`]. A type used by three
    /// operations belongs here once: inlined three times it triples the
    /// document and a client generator emits three identical models.
    #[must_use]
    pub fn component(mut self, name: impl Into<String>, schema: Schema) -> Self {
        if let Err(error) = self.document.components.schema(name, schema) {
            self.errors.push(error.into());
        }
        self
    }

    /// Registers a [`SchemaSource`] and everything it depends on.
    #[must_use]
    pub fn component_of<T: SchemaSource>(mut self) -> Self {
        if let Err(error) = self.document.components.schema_of::<T>() {
            self.errors.push(error.into());
        }
        self
    }

    /// Registers a named schema.
    #[must_use]
    pub fn named_component(mut self, named: NamedSchema) -> Self {
        if let Err(error) = self.document.components.named(named) {
            self.errors.push(error.into());
        }
        self
    }

    /// Whether implicit HEAD operations are emitted.
    #[must_use]
    pub fn head_policy(mut self, policy: HeadPolicy) -> Self {
        self.head_policy = policy;
        self
    }

    /// How a generated `operationId` is spelled.
    #[must_use]
    pub fn operation_id_style(mut self, style: OperationIdStyle) -> Self {
        self.id_style = style;
        self
    }

    /// Adds one route.
    #[must_use]
    pub fn route(mut self, template: &str, verb: Verb) -> Self {
        match RouteTemplate::parse(template) {
            Ok(parsed) => self.routes.push(RouteEntry {
                template: parsed,
                verb,
            }),
            Err(source) => self.errors.push(SpecError::Template {
                template: template.to_owned(),
                source,
            }),
        }
        self
    }

    /// Adds one route from an already parsed template.
    #[must_use]
    pub fn route_template(mut self, template: RouteTemplate, verb: Verb) -> Self {
        self.routes.push(RouteEntry { template, verb });
        self
    }

    /// Adds every route the router answers.
    ///
    /// This is the point of the crate: the table the server dispatches on is
    /// the table the document describes, so the two cannot disagree.
    #[must_use]
    pub fn routes_from_router<H>(mut self, router: &Router<H>) -> Self {
        for (template, allow) in router.routes() {
            let has_get = allow.contains(Verb::Get);
            for verb in allow.iter() {
                // The router adds HEAD to every GET, as RFC 9110 requires.
                // That one is the server's business, not the document's.
                if verb == Verb::Head && has_get && self.head_policy == HeadPolicy::Skip {
                    continue;
                }
                self.routes.push(RouteEntry {
                    template: template.clone(),
                    verb,
                });
            }
        }
        self
    }

    /// Annotates one operation.
    ///
    /// The template may be written either way round for a catch-all:
    /// `/docs/{*rest}` as the router spells it, or `/docs/{rest}` as the
    /// document does.
    #[must_use]
    pub fn describe<F>(mut self, template: &str, verb: Verb, annotate: F) -> Self
    where
        F: FnOnce(OperationSpec) -> OperationSpec,
    {
        let key = openapi_path(template);
        let existing = self.annotations.remove(&(key.clone(), verb));
        self.annotations
            .insert((key, verb), annotate(existing.unwrap_or_default()));
        self
    }

    /// Annotates one operation from a handler type.
    #[must_use]
    pub fn describe_with<T: DescribeOperation>(self, template: &str, verb: Verb) -> Self {
        self.describe(template, verb, |_| T::describe())
    }

    /// Hides one route from the document without removing it from the table.
    #[must_use]
    pub fn hide(self, template: &str, verb: Verb) -> Self {
        self.describe(template, verb, OperationSpec::hidden)
    }

    /// Derives the document.
    ///
    /// # Errors
    ///
    /// [`SpecError`] for a route that cannot be described, a collision this
    /// crate must not paper over, or — and this one should never fire — a
    /// derived document that does not validate.
    pub fn build(mut self) -> Result<Document, SpecError> {
        if let Some(error) = self.errors.first() {
            return Err(error.clone());
        }

        let mut seen: Vec<(String, Verb)> = Vec::new();
        for entry in &self.routes {
            let key = (entry.template.pattern().to_owned(), entry.verb);
            if seen.contains(&key) {
                return Err(SpecError::DuplicateRoute {
                    verb: entry.verb,
                    template: entry.template.pattern().to_owned(),
                });
            }
            seen.push(key);
        }

        // Every annotation must describe a route that exists. A typo that
        // silently documents nothing is the failure mode of every annotation
        // system that does not check this.
        let described: Vec<String> = self
            .routes
            .iter()
            .map(|entry| openapi_path(entry.template.pattern()))
            .collect();
        for (path, verb) in self.annotations.keys() {
            let exists = self
                .routes
                .iter()
                .any(|entry| entry.verb == *verb && openapi_path(entry.template.pattern()) == *path);
            if !exists {
                return Err(SpecError::UnknownRoute {
                    verb: *verb,
                    template: path.clone(),
                    suggestion: nearest_name(path, &described),
                });
            }
        }

        // Two templates that differ only in their parameter names are one
        // path to OpenAPI. Emitting both would produce a document whose
        // second entry silently replaces the first in most parsers.
        let mut shapes: Vec<(String, String)> = Vec::new();
        for entry in &self.routes {
            let path = openapi_path(entry.template.pattern());
            let shape = path_shape(&path);
            if let Some((other, _)) = shapes
                .iter()
                .find(|(other, existing)| *existing == shape && *other != path)
            {
                return Err(SpecError::AmbiguousPaths {
                    first: other.clone(),
                    second: path,
                });
            }
            if !shapes.iter().any(|(p, _)| *p == path) {
                shapes.push((path, shape));
            }
        }

        let mut paths: Vec<(String, PathItem)> = Vec::new();
        let mut ids: Vec<(String, String)> = Vec::new();
        let mut used_tags: Vec<String> = Vec::new();

        for entry in &self.routes {
            let path = openapi_path(entry.template.pattern());
            let annotation = self
                .annotations
                .get(&(path.clone(), entry.verb))
                .cloned()
                .unwrap_or_default();
            if annotation.hidden {
                continue;
            }

            let operation = self.operation_for(entry, &annotation)?;
            let at = format!("{} {}", entry.verb.as_str(), path);
            if let Some((_, first)) = ids.iter().find(|(id, _)| *id == operation.operation_id) {
                return Err(SpecError::DuplicateOperationId {
                    id: operation.operation_id.clone(),
                    first: first.clone(),
                    second: at,
                });
            }
            ids.push((operation.operation_id.clone(), at));
            for tag in &operation.tags {
                if !used_tags.contains(tag) {
                    used_tags.push(tag.clone());
                }
            }

            match paths.iter_mut().find(|(key, _)| *key == path) {
                Some((_, item)) => item.operations.push((entry.verb, operation)),
                None => {
                    let mut item = PathItem::default();
                    item.operations.push((entry.verb, operation));
                    paths.push((path, item));
                }
            }
        }

        for tag in used_tags {
            if !self.document.tags.iter().any(|t| t.name == tag) {
                self.document.tags.push(Tag::new(tag));
            }
        }

        self.document.paths = paths;
        self.document.validate()?;
        Ok(self.document)
    }

    fn operation_for(
        &self,
        entry: &RouteEntry,
        annotation: &OperationSpec,
    ) -> Result<Operation, SpecError> {
        let operation_id = annotation
            .operation_id
            .clone()
            .unwrap_or_else(|| operation_id_for(entry.verb, &entry.template, self.id_style));
        let mut operation = Operation::new(operation_id);

        operation.summary = annotation.summary.clone();
        operation.description = annotation.description.clone();
        operation.deprecated = annotation.deprecated;
        operation.security = annotation.security.clone();
        operation.external_docs = annotation.external_docs.clone();
        operation.request_body = annotation.request_body.clone();

        operation.tags = if annotation.tags.is_empty() {
            default_tag(&entry.template).into_iter().collect()
        } else {
            annotation.tags.clone()
        };

        // Path parameters come from the route declaration. There is nothing to
        // guess and nothing to forget: a parameter in the template is a
        // parameter in the document, always.
        let declared: Vec<&str> = entry
            .template
            .params()
            .iter()
            .map(brocanth::routing::ParamSpec::name)
            .collect();
        for (name, _, _) in &annotation.path_params {
            if !declared.contains(&name.as_str()) {
                return Err(SpecError::UnknownPathParameter {
                    verb: entry.verb,
                    template: entry.template.pattern().to_owned(),
                    name: name.clone(),
                    suggestion: nearest_name(name, &declared),
                });
            }
        }

        for spec in entry.template.params() {
            let refinement = annotation
                .path_params
                .iter()
                .find(|(name, _, _)| name == spec.name());
            let (schema, description) = match refinement {
                Some((_, schema, description)) => (schema.clone(), description.clone()),
                None => (
                    Schema::string(),
                    match spec.kind() {
                        ParamKind::CatchAll => Some(
                            "Matches the rest of the path and may contain `/`.".to_owned(),
                        ),
                        ParamKind::Dynamic => None,
                    },
                ),
            };
            let mut parameter = Parameter::path(spec.name(), schema);
            parameter.description = description;
            operation.parameters.push(parameter);
        }

        operation
            .parameters
            .extend(annotation.parameters.iter().cloned());

        if !annotation.responses.is_empty() {
            operation.responses = annotation.responses.clone();
        }

        Ok(operation)
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

/// A brocanth template as an OpenAPI path key.
///
/// `/docs/{*rest}` becomes `/docs/{rest}`: OpenAPI has no catch-all syntax,
/// so the star is dropped and the parameter's description carries the fact
/// that it may contain a slash.
#[must_use]
pub fn openapi_path(template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..=open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(after);
            return out;
        };
        let name = after[..close].strip_prefix('*').unwrap_or(&after[..close]);
        out.push_str(name);
        out.push('}');
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A path with its parameter names removed, which is what OpenAPI compares
/// two paths by.
fn path_shape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        out.push_str("{}");
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => rest = &after[close + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The tag an operation gets when nobody names one: the first literal
/// segment of its path.
fn default_tag(template: &RouteTemplate) -> Option<String> {
    template
        .pattern()
        .split('/')
        .filter(|segment| !segment.is_empty() && !segment.starts_with('{'))
        .map(|segment| segment.to_lowercase())
        .next()
}

/// The `operationId` for a route that does not name one.
///
/// Derived from the verb and the template, so it is stable across runs and
/// changes only when the route does — which is the behaviour drift detection
/// depends on.
#[must_use]
pub fn operation_id_for(verb: Verb, template: &RouteTemplate, style: OperationIdStyle) -> String {
    let mut words: Vec<String> = vec![verb.as_str().to_lowercase()];
    for segment in template.pattern().split('/').filter(|s| !s.is_empty()) {
        if let Some(inner) = segment.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            words.push("by".to_owned());
            words.push(sanitize(inner.strip_prefix('*').unwrap_or(inner)));
        } else {
            words.push(sanitize(segment));
        }
    }
    if words.len() == 1 {
        words.push("root".to_owned());
    }
    match style {
        OperationIdStyle::Snake => words.join("_"),
        OperationIdStyle::Camel => {
            let mut out = String::new();
            for (index, word) in words.iter().enumerate() {
                if index == 0 {
                    out.push_str(word);
                } else {
                    let mut chars = word.chars();
                    if let Some(first) = chars.next() {
                        out.extend(first.to_uppercase());
                        out.push_str(chars.as_str());
                    }
                }
            }
            out
        }
    }
}

fn sanitize(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    let mut last_was_underscore = false;
    for ch in segment.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_underscore = false;
        } else if !last_was_underscore && !out.is_empty() {
            out.push('_');
            last_was_underscore = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("segment");
    }
    out
}

/// Checks that a document's paths and parameters agree, for a caller that
/// built a [`Document`] by hand rather than through [`SpecBuilder`].
///
/// # Errors
///
/// A message per path whose template and parameters disagree.
pub fn check_path_parameters(document: &Document) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    for (path, item) in &document.paths {
        let variables = match path_variables(path) {
            Ok(variables) => variables,
            Err(message) => {
                problems.push(format!("{path}: {message}"));
                continue;
            }
        };
        for (verb, operation) in &item.operations {
            for name in &variables {
                let declared = item
                    .parameters
                    .iter()
                    .chain(operation.parameters.iter())
                    .any(|p| p.location == ParamIn::Path && p.name == *name);
                if !declared {
                    problems.push(format!(
                        "{path} {}: `{{{name}}}` is in the path and not in `parameters`",
                        verb.as_str()
                    ));
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// An example value, for an annotation that wants one.
#[must_use]
pub fn example(value: impl Into<Json>) -> Json {
    value.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_catch_all_loses_its_star_in_the_path_key() {
        assert_eq!(openapi_path("/docs/{*rest}"), "/docs/{rest}");
        assert_eq!(openapi_path("/users/{id}"), "/users/{id}");
        assert_eq!(openapi_path("/"), "/");
    }

    #[test]
    fn operation_ids_are_derived_from_the_route() {
        let cases = [
            ("/", Verb::Get, "get_root"),
            ("/posts", Verb::Get, "get_posts"),
            ("/posts/{id}", Verb::Delete, "delete_posts_by_id"),
            ("/posts/{id}/comments", Verb::Post, "post_posts_by_id_comments"),
            ("/docs/{*rest}", Verb::Get, "get_docs_by_rest"),
        ];
        for (pattern, verb, expected) in cases {
            let template = RouteTemplate::parse(pattern).unwrap();
            assert_eq!(
                operation_id_for(verb, &template, OperationIdStyle::Snake),
                expected
            );
        }
    }

    #[test]
    fn camel_case_operation_ids() {
        let template = RouteTemplate::parse("/posts/{id}").unwrap();
        assert_eq!(
            operation_id_for(Verb::Get, &template, OperationIdStyle::Camel),
            "getPostsById"
        );
    }

    #[test]
    fn path_shapes_ignore_parameter_names() {
        assert_eq!(path_shape("/users/{id}"), path_shape("/users/{userId}"));
        assert_ne!(path_shape("/users/{id}"), path_shape("/users/{id}/edit"));
    }

    #[test]
    fn the_default_tag_is_the_first_literal_segment() {
        let template = RouteTemplate::parse("/posts/{id}").unwrap();
        assert_eq!(default_tag(&template), Some("posts".to_owned()));
        let root = RouteTemplate::parse("/").unwrap();
        assert_eq!(default_tag(&root), None);
    }
}
