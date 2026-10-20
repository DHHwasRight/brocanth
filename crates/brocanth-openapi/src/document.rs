//! The document model, and the structural check that keeps it honest.
//!
//! Everything here is a plain value that knows how to turn itself into
//! [`Json`]. Nothing reads the filesystem, nothing holds a route table, and
//! nothing is derived — [`crate::spec`] does the deriving and hands the
//! result over. That split is what lets a hand-built document be tested the
//! same way a generated one is.
//!
//! # Key order
//!
//! Emission order is fixed and documented rather than incidental: the order
//! the OpenAPI specification lists the fields in, and component schemas
//! sorted by name. A generated document is a file somebody commits, so a
//! field that moves between runs is a diff nobody can read.

use std::collections::BTreeMap;

use brocanth::routing::Verb;

use crate::json::Json;
use crate::schema::{NamedSchema, Schema, SchemaSource};

/// The version this crate emits.
pub const OPENAPI_VERSION: &str = "3.1.0";

// ---------------------------------------------------------------------------
// Info
// ---------------------------------------------------------------------------

/// Who to contact about the API.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contact {
    /// A name.
    pub name: Option<String>,
    /// A URL.
    pub url: Option<String>,
    /// An email address.
    pub email: Option<String>,
}

/// The licence the API is published under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct License {
    /// The licence name.
    pub name: String,
    /// An SPDX identifier, which 3.1 added and prefers over a URL.
    pub identifier: Option<String>,
    /// A URL. Mutually exclusive with `identifier`.
    pub url: Option<String>,
}

/// The `info` object. `title` and `version` are the two required fields of
/// the whole document.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// The API title.
    pub title: String,
    /// The API version. Not the OpenAPI version.
    pub version: String,
    /// A one-line summary, added in 3.1.
    pub summary: Option<String>,
    /// A longer description. CommonMark.
    pub description: Option<String>,
    /// A terms-of-service URL.
    pub terms_of_service: Option<String>,
    /// Contact details.
    pub contact: Option<Contact>,
    /// Licence.
    pub license: Option<License>,
}

impl Info {
    /// The two required fields.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            version: version.into(),
            ..Self::default()
        }
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("title", &self.title);
        obj.maybe("summary", self.summary.as_ref());
        obj.maybe("description", self.description.as_ref());
        obj.maybe("termsOfService", self.terms_of_service.as_ref());
        if let Some(contact) = &self.contact {
            let mut c = Json::object();
            c.maybe("name", contact.name.as_ref());
            c.maybe("url", contact.url.as_ref());
            c.maybe("email", contact.email.as_ref());
            if !c.is_empty() {
                obj.set("contact", c.build());
            }
        }
        if let Some(license) = &self.license {
            let mut l = Json::object();
            l.set("name", &license.name);
            l.maybe("identifier", license.identifier.as_ref());
            l.maybe("url", license.url.as_ref());
            obj.set("license", l.build());
        }
        obj.set("version", &self.version);
        obj.build()
    }
}

// ---------------------------------------------------------------------------
// Servers, tags, external docs
// ---------------------------------------------------------------------------

/// One entry in `servers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Server {
    /// The base URL. May be relative, and `/` is the honest default for an
    /// application that does not know where it is deployed.
    pub url: String,
    /// What this server is.
    pub description: Option<String>,
}

impl Server {
    /// A server with no description.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            description: None,
        }
    }

    /// A described server.
    #[must_use]
    pub fn described(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("url", &self.url);
        obj.maybe("description", self.description.as_ref());
        obj.build()
    }
}

/// One entry in the top-level `tags`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The tag name, as operations spell it.
    pub name: String,
    /// What the tag groups.
    pub description: Option<String>,
}

impl Tag {
    /// A tag with no description.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: None,
        }
    }

    /// A described tag.
    #[must_use]
    pub fn described(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("name", &self.name);
        obj.maybe("description", self.description.as_ref());
        obj.build()
    }
}

/// A link to documentation outside the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalDocs {
    /// Where to go.
    pub url: String,
    /// What is there.
    pub description: Option<String>,
}

impl ExternalDocs {
    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.maybe("description", self.description.as_ref());
        obj.set("url", &self.url);
        obj.build()
    }
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// Where an API key travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiKeyIn {
    /// A query parameter.
    Query,
    /// A header.
    Header,
    /// A cookie.
    Cookie,
}

impl ApiKeyIn {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }
}

/// One entry in `components/securitySchemes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecurityScheme {
    /// `type: apiKey`.
    ApiKey {
        /// The parameter or header name.
        name: String,
        /// Where it travels.
        location: ApiKeyIn,
        /// What it is.
        description: Option<String>,
    },
    /// `type: http`, such as `basic` or `bearer`.
    Http {
        /// The HTTP authentication scheme, lowercase per RFC 7235.
        scheme: String,
        /// A hint for a bearer token's format, such as `JWT`.
        bearer_format: Option<String>,
        /// What it is.
        description: Option<String>,
    },
    /// `type: openIdConnect`.
    OpenIdConnect {
        /// The discovery URL.
        url: String,
        /// What it is.
        description: Option<String>,
    },
}

impl SecurityScheme {
    /// A bearer-token scheme.
    pub fn bearer(format: impl Into<String>) -> Self {
        Self::Http {
            scheme: "bearer".to_owned(),
            bearer_format: Some(format.into()),
            description: None,
        }
    }

    /// A session-cookie scheme, which is what an HTML-first application
    /// actually uses.
    pub fn session_cookie(name: impl Into<String>) -> Self {
        Self::ApiKey {
            name: name.into(),
            location: ApiKeyIn::Cookie,
            description: Some("Session cookie set at sign-in.".to_owned()),
        }
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        match self {
            Self::ApiKey {
                name,
                location,
                description,
            } => {
                obj.set("type", "apiKey");
                obj.maybe("description", description.as_ref());
                obj.set("name", name);
                obj.set("in", location.as_str());
            }
            Self::Http {
                scheme,
                bearer_format,
                description,
            } => {
                obj.set("type", "http");
                obj.maybe("description", description.as_ref());
                obj.set("scheme", scheme);
                obj.maybe("bearerFormat", bearer_format.as_ref());
            }
            Self::OpenIdConnect { url, description } => {
                obj.set("type", "openIdConnect");
                obj.maybe("description", description.as_ref());
                obj.set("openIdConnectUrl", url);
            }
        }
        obj.build()
    }
}

/// One alternative in a `security` list: a scheme and the scopes it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecurityRequirement {
    /// The scheme name, as registered in `components/securitySchemes`.
    pub scheme: String,
    /// Scopes. Empty for everything that is not OAuth2 or OpenID Connect.
    pub scopes: Vec<String>,
}

impl SecurityRequirement {
    /// A requirement with no scopes.
    pub fn new(scheme: impl Into<String>) -> Self {
        Self {
            scheme: scheme.into(),
            scopes: Vec::new(),
        }
    }

    fn to_json(&self) -> Json {
        let scopes: Vec<Json> = self.scopes.iter().map(|s| Json::Str(s.clone())).collect();
        Json::Object(vec![(self.scheme.clone(), Json::Array(scopes))])
    }
}

// ---------------------------------------------------------------------------
// Parameters, bodies, responses
// ---------------------------------------------------------------------------

/// Where a parameter travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ParamIn {
    /// Part of the path template. Always required.
    Path,
    /// After the `?`.
    Query,
    /// A request header.
    Header,
    /// A cookie.
    Cookie,
}

impl ParamIn {
    /// The keyword, as it appears in the document.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }
}

/// One entry in an operation's `parameters`.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
    /// The name. For a path parameter it must match the template exactly.
    pub name: String,
    /// Where it travels.
    pub location: ParamIn,
    /// What it is.
    pub description: Option<String>,
    /// Whether it must be present. Forced true for a path parameter.
    pub required: bool,
    /// Whether it is on its way out.
    pub deprecated: bool,
    /// Its type.
    pub schema: Schema,
    /// An example value.
    pub example: Option<Json>,
}

impl Parameter {
    /// A parameter.
    pub fn new(name: impl Into<String>, location: ParamIn, schema: Schema) -> Self {
        Self {
            name: name.into(),
            location,
            description: None,
            required: location == ParamIn::Path,
            deprecated: false,
            schema,
            example: None,
        }
    }

    /// A path parameter. Required, because the path does not match without it.
    pub fn path(name: impl Into<String>, schema: Schema) -> Self {
        Self::new(name, ParamIn::Path, schema)
    }

    /// A query parameter, optional by default.
    pub fn query(name: impl Into<String>, schema: Schema) -> Self {
        Self::new(name, ParamIn::Query, schema)
    }

    /// Describes the parameter.
    #[must_use]
    pub fn described(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Marks the parameter required.
    #[must_use]
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Marks the parameter deprecated.
    #[must_use]
    pub fn deprecated(mut self) -> Self {
        self.deprecated = true;
        self
    }

    /// Gives the parameter an example.
    #[must_use]
    pub fn example(mut self, value: impl Into<Json>) -> Self {
        self.example = Some(value.into());
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("name", &self.name);
        obj.set("in", self.location.as_str());
        obj.maybe("description", self.description.as_ref());
        if self.location == ParamIn::Path || self.required {
            obj.set("required", true);
        }
        obj.flag("deprecated", self.deprecated);
        obj.set("schema", self.schema.to_json());
        obj.maybe("example", self.example.clone());
        obj.build()
    }
}

/// One `content` entry: a media type and the schema of a body in it.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaType {
    /// The schema of the body.
    pub schema: Schema,
    /// An example body.
    pub example: Option<Json>,
}

impl MediaType {
    /// A media type with no example.
    #[must_use]
    pub fn new(schema: Schema) -> Self {
        Self {
            schema,
            example: None,
        }
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("schema", self.schema.to_json());
        obj.maybe("example", self.example.clone());
        obj.build()
    }
}

/// An operation's `requestBody`.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestBody {
    /// What the body is.
    pub description: Option<String>,
    /// Whether it must be sent.
    pub required: bool,
    /// One entry per media type, in insertion order.
    pub content: Vec<(String, MediaType)>,
}

impl RequestBody {
    /// A required body in one media type.
    pub fn of(media_type: impl Into<String>, schema: Schema) -> Self {
        Self {
            description: None,
            required: true,
            content: vec![(media_type.into(), MediaType::new(schema))],
        }
    }

    /// A JSON body.
    #[must_use]
    pub fn json(schema: Schema) -> Self {
        Self::of("application/json", schema)
    }

    /// A form body, which is what an HTML-first application posts.
    #[must_use]
    pub fn form(schema: Schema) -> Self {
        Self::of("application/x-www-form-urlencoded", schema)
    }

    /// Describes the body.
    #[must_use]
    pub fn described(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    /// Marks the body optional.
    #[must_use]
    pub fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Adds another media type carrying the same or a different schema.
    #[must_use]
    pub fn also(mut self, media_type: impl Into<String>, schema: Schema) -> Self {
        self.content
            .push((media_type.into(), MediaType::new(schema)));
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.maybe("description", self.description.as_ref());
        obj.set("content", content_json(&self.content));
        obj.flag("required", self.required);
        obj.build()
    }
}

/// One response header.
#[derive(Debug, Clone, PartialEq)]
pub struct Header {
    /// What it means.
    pub description: Option<String>,
    /// Its type.
    pub schema: Schema,
}

impl Header {
    /// A header of a given type.
    #[must_use]
    pub fn new(schema: Schema) -> Self {
        Self {
            description: None,
            schema,
        }
    }

    /// A described header.
    #[must_use]
    pub fn described(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.maybe("description", self.description.as_ref());
        obj.set("schema", self.schema.to_json());
        obj.build()
    }
}

/// One entry in an operation's `responses`.
///
/// `description` is required by the specification, which is why it is not an
/// [`Option`]: a response with no description is not a valid document, so
/// there is no way to build one here.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// What this status means for this operation.
    pub description: String,
    /// Headers this response sets.
    pub headers: Vec<(String, Header)>,
    /// Bodies, one per media type.
    pub content: Vec<(String, MediaType)>,
}

impl Response {
    /// A response with no body.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            headers: Vec::new(),
            content: Vec::new(),
        }
    }

    /// A response with a JSON body.
    pub fn json(description: impl Into<String>, schema: Schema) -> Self {
        Self::new(description).with_content("application/json", schema)
    }

    /// A response with an HTML body, which most of this framework's
    /// handlers return.
    pub fn html(description: impl Into<String>) -> Self {
        Self::new(description).with_content("text/html", Schema::string())
    }

    /// Adds a body in a media type.
    #[must_use]
    pub fn with_content(mut self, media_type: impl Into<String>, schema: Schema) -> Self {
        self.content
            .push((media_type.into(), MediaType::new(schema)));
        self
    }

    /// Adds a response header.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, header: Header) -> Self {
        self.headers.push((name.into(), header));
        self
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("description", &self.description);
        if !self.headers.is_empty() {
            let mut headers = Json::object();
            for (name, header) in &self.headers {
                headers.set(name, header.to_json());
            }
            obj.set("headers", headers.build());
        }
        if !self.content.is_empty() {
            obj.set("content", content_json(&self.content));
        }
        obj.build()
    }
}

fn content_json(content: &[(String, MediaType)]) -> Json {
    let mut obj = Json::object();
    for (media_type, body) in content {
        obj.set(media_type, body.to_json());
    }
    obj.build()
}

// ---------------------------------------------------------------------------
// Operations and paths
// ---------------------------------------------------------------------------

/// One operation: a verb on a path.
#[derive(Debug, Clone, PartialEq)]
pub struct Operation {
    /// Unique across the whole document. A generator turns it into a method
    /// name, so a collision silently loses one of the two.
    pub operation_id: String,
    /// One line.
    pub summary: Option<String>,
    /// More than one line. CommonMark.
    pub description: Option<String>,
    /// Groups the operation belongs to.
    pub tags: Vec<String>,
    /// Whether it is on its way out.
    pub deprecated: bool,
    /// Path, query, header and cookie parameters.
    pub parameters: Vec<Parameter>,
    /// The request body, for the verbs that take one.
    pub request_body: Option<RequestBody>,
    /// Responses by status code, or `default`. Never empty.
    pub responses: Vec<(String, Response)>,
    /// Overrides the document-level `security`. `Some(vec![])` means this
    /// operation is public in an otherwise protected API.
    pub security: Option<Vec<SecurityRequirement>>,
    /// A link out.
    pub external_docs: Option<ExternalDocs>,
}

impl Operation {
    /// An operation with one unnamed 200 response.
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            summary: None,
            description: None,
            tags: Vec::new(),
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            responses: vec![("200".to_owned(), Response::new("OK"))],
            security: None,
            external_docs: None,
        }
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.list(
            "tags",
            self.tags.iter().map(|t| Json::Str(t.clone())).collect(),
        );
        obj.maybe("summary", self.summary.as_ref());
        obj.maybe("description", self.description.as_ref());
        if let Some(docs) = &self.external_docs {
            obj.set("externalDocs", docs.to_json());
        }
        obj.set("operationId", &self.operation_id);
        obj.list(
            "parameters",
            self.parameters.iter().map(Parameter::to_json).collect(),
        );
        if let Some(body) = &self.request_body {
            obj.set("requestBody", body.to_json());
        }
        let mut responses = Json::object();
        for (status, response) in &self.responses {
            responses.set(status, response.to_json());
        }
        obj.set("responses", responses.build());
        obj.flag("deprecated", self.deprecated);
        if let Some(security) = &self.security {
            obj.set(
                "security",
                security
                    .iter()
                    .map(SecurityRequirement::to_json)
                    .collect::<Vec<_>>(),
            );
        }
        obj.build()
    }
}

/// Every operation on one path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PathItem {
    /// One line about the path itself.
    pub summary: Option<String>,
    /// More than one line about the path itself.
    pub description: Option<String>,
    /// Parameters shared by every operation on the path.
    pub parameters: Vec<Parameter>,
    /// Operations, in registration order. Emitted in the specification's
    /// order.
    pub operations: Vec<(Verb, Operation)>,
}

impl PathItem {
    /// The operation for `verb`, if the path answers it.
    #[must_use]
    pub fn operation(&self, verb: Verb) -> Option<&Operation> {
        self.operations
            .iter()
            .find(|(v, _)| *v == verb)
            .map(|(_, op)| op)
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.maybe("summary", self.summary.as_ref());
        obj.maybe("description", self.description.as_ref());
        for verb in VERB_ORDER {
            if let Some(operation) = self.operation(verb) {
                obj.set(&verb.as_str().to_lowercase(), operation.to_json());
            }
        }
        obj.list(
            "parameters",
            self.parameters.iter().map(Parameter::to_json).collect(),
        );
        obj.build()
    }
}

/// The order OpenAPI lists verbs on a path item in.
const VERB_ORDER: [Verb; 7] = [
    Verb::Get,
    Verb::Put,
    Verb::Post,
    Verb::Delete,
    Verb::Options,
    Verb::Head,
    Verb::Patch,
];

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// A component that cannot be registered.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ComponentError {
    /// Two different schemas want the same component name.
    ///
    /// Overwriting would make the surviving definition depend on
    /// registration order, which is the kind of failure that shows up as a
    /// wrong field in a generated client three weeks later.
    #[error(
        "the component schema `{name}` is already registered with a different definition; \
         two types cannot share one name"
    )]
    SchemaConflict {
        /// The contested name.
        name: String,
    },

    /// Two different schemes want the same security scheme name.
    #[error("the security scheme `{name}` is already registered with a different definition")]
    SecurityConflict {
        /// The contested name.
        name: String,
    },

    /// A component name is not usable in a `$ref`.
    #[error(
        "`{name}` is not a usable component name; it must match `[A-Za-z0-9._-]+`, because the \
         name goes into a JSON pointer"
    )]
    BadName {
        /// The rejected name.
        name: String,
    },
}

/// `components`: the definitions that make a document small.
///
/// A type used by three operations belongs here once and is referenced three
/// times. Inlining it instead triples the document and makes a client
/// generator emit three identical models with three different names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Components {
    schemas: BTreeMap<String, Schema>,
    security_schemes: BTreeMap<String, SecurityScheme>,
}

impl Components {
    /// No components.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a schema and returns a reference to it.
    ///
    /// Registering the same name with an equal schema is a no-op, so the same
    /// type may be registered by every operation that uses it.
    ///
    /// # Errors
    ///
    /// [`ComponentError::SchemaConflict`] when the name is taken by a
    /// different definition, and [`ComponentError::BadName`] when the name
    /// cannot go into a JSON pointer.
    pub fn schema(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
    ) -> Result<Schema, ComponentError> {
        let name = name.into();
        if !is_component_name(&name) {
            return Err(ComponentError::BadName { name });
        }
        match self.schemas.get(&name) {
            Some(existing) if *existing != schema => {
                return Err(ComponentError::SchemaConflict { name })
            }
            Some(_) => {}
            None => {
                self.schemas.insert(name.clone(), schema);
            }
        }
        Ok(Schema::reference(name))
    }

    /// Registers a [`SchemaSource`] and its dependencies.
    ///
    /// Returns a reference when the type named itself, and the schema inline
    /// when it did not.
    ///
    /// # Errors
    ///
    /// As [`Components::schema`].
    pub fn schema_of<T: SchemaSource>(&mut self) -> Result<Schema, ComponentError> {
        for dependency in T::dependencies() {
            self.schema(dependency.name, dependency.schema)?;
        }
        match T::schema_name() {
            Some(name) => self.schema(name, T::schema()),
            None => Ok(T::schema()),
        }
    }

    /// Registers a named schema.
    ///
    /// # Errors
    ///
    /// As [`Components::schema`].
    pub fn named(&mut self, named: NamedSchema) -> Result<Schema, ComponentError> {
        self.schema(named.name, named.schema)
    }

    /// Registers a security scheme.
    ///
    /// # Errors
    ///
    /// [`ComponentError::SecurityConflict`] when the name is taken by a
    /// different definition.
    pub fn security_scheme(
        &mut self,
        name: impl Into<String>,
        scheme: SecurityScheme,
    ) -> Result<(), ComponentError> {
        let name = name.into();
        if !is_component_name(&name) {
            return Err(ComponentError::BadName { name });
        }
        match self.security_schemes.get(&name) {
            Some(existing) if *existing != scheme => Err(ComponentError::SecurityConflict { name }),
            Some(_) => Ok(()),
            None => {
                self.security_schemes.insert(name, scheme);
                Ok(())
            }
        }
    }

    /// The registered schema names, sorted.
    pub fn schema_names(&self) -> impl Iterator<Item = &str> {
        self.schemas.keys().map(String::as_str)
    }

    /// One registered schema.
    #[must_use]
    pub fn get_schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.get(name)
    }

    /// Whether a security scheme is registered.
    #[must_use]
    pub fn has_security_scheme(&self, name: &str) -> bool {
        self.security_schemes.contains_key(name)
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.schemas.is_empty() && self.security_schemes.is_empty()
    }

    fn to_json(&self) -> Json {
        let mut obj = Json::object();
        if !self.schemas.is_empty() {
            let mut schemas = Json::object();
            for (name, schema) in &self.schemas {
                schemas.set(name, schema.to_json());
            }
            obj.set("schemas", schemas.build());
        }
        if !self.security_schemes.is_empty() {
            let mut schemes = Json::object();
            for (name, scheme) in &self.security_schemes {
                schemes.set(name, scheme.to_json());
            }
            obj.set("securitySchemes", schemes.build());
        }
        obj.build()
    }
}

fn is_component_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

/// A complete OpenAPI 3.1 document.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// The specification version. `3.1.0`.
    pub openapi: String,
    /// Title, version and the rest.
    pub info: Info,
    /// Where the API is served from.
    pub servers: Vec<Server>,
    /// Paths, in the order they were added.
    pub paths: Vec<(String, PathItem)>,
    /// Reusable definitions.
    pub components: Components,
    /// Tag descriptions.
    pub tags: Vec<Tag>,
    /// What every operation requires unless it says otherwise.
    pub security: Vec<SecurityRequirement>,
    /// A link out.
    pub external_docs: Option<ExternalDocs>,
}

impl Document {
    /// An empty document with the two required `info` fields.
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            openapi: OPENAPI_VERSION.to_owned(),
            info: Info::new(title, version),
            servers: vec![Server::new("/")],
            paths: Vec::new(),
            components: Components::new(),
            tags: Vec::new(),
            security: Vec::new(),
            external_docs: None,
        }
    }

    /// The path item for `path`.
    #[must_use]
    pub fn path(&self, path: &str) -> Option<&PathItem> {
        self.paths
            .iter()
            .find(|(key, _)| key == path)
            .map(|(_, item)| item)
    }

    /// Every operation in the document, with the path and verb it sits on.
    pub fn operations(&self) -> impl Iterator<Item = (&str, Verb, &Operation)> {
        self.paths.iter().flat_map(|(path, item)| {
            item.operations
                .iter()
                .map(move |(verb, op)| (path.as_str(), *verb, op))
        })
    }

    /// How many operations the document describes.
    #[must_use]
    pub fn operation_count(&self) -> usize {
        self.paths
            .iter()
            .map(|(_, item)| item.operations.len())
            .sum()
    }

    /// The document as a value, ready to write.
    #[must_use]
    pub fn to_json(&self) -> Json {
        let mut obj = Json::object();
        obj.set("openapi", &self.openapi);
        obj.set("info", self.info.to_json());
        obj.list(
            "servers",
            self.servers.iter().map(Server::to_json).collect(),
        );
        let mut paths = Json::object();
        for (path, item) in &self.paths {
            paths.set(path, item.to_json());
        }
        obj.set("paths", paths.build());
        if !self.components.is_empty() {
            obj.set("components", self.components.to_json());
        }
        if !self.security.is_empty() {
            obj.set(
                "security",
                self.security
                    .iter()
                    .map(SecurityRequirement::to_json)
                    .collect::<Vec<_>>(),
            );
        }
        obj.list("tags", self.tags.iter().map(Tag::to_json).collect());
        if let Some(docs) = &self.external_docs {
            obj.set("externalDocs", docs.to_json());
        }
        obj.build()
    }

    /// The document as indented JSON, with a trailing newline.
    #[must_use]
    pub fn to_json_string(&self) -> String {
        let mut text = self.to_json().to_json();
        text.push('\n');
        text
    }

    /// The document as block-style YAML, with a trailing newline.
    #[must_use]
    pub fn to_yaml_string(&self) -> String {
        let mut text = self.to_json().to_yaml();
        text.push('\n');
        text
    }

    /// Checks the document against the structural rules of OpenAPI 3.1.
    ///
    /// This is a hand-written check rather than a meta-schema validator, and
    /// the trade is deliberate: a JSON Schema 2020-12 validator is a large
    /// dependency for a crate whose whole argument is that it adds nothing to
    /// an application's tree. What it covers is every rule this generator
    /// could plausibly break, and each of those is a failure that no tool
    /// reports at the point it is made:
    ///
    /// * a path parameter in the URL that is missing from `parameters`, and a
    ///   path parameter in `parameters` that is missing from the URL;
    /// * a duplicate `operationId`, which a client generator resolves by
    ///   dropping one of the two;
    /// * a `$ref` to a component that is not defined;
    /// * a `required` entry naming a property that is not declared;
    /// * a `security` entry naming a scheme that is not defined;
    /// * an operation with no responses, or a response key that is not a
    ///   status code.
    ///
    /// # Errors
    ///
    /// [`Validation`] lists every problem found, not only the first.
    pub fn validate(&self) -> Result<(), Validation> {
        let mut problems = Vec::new();

        if !self.openapi.starts_with("3.1") {
            problems.push(format!(
                "openapi: `{}` is not a 3.1 version; this crate emits {OPENAPI_VERSION}",
                self.openapi
            ));
        }
        if self.info.title.trim().is_empty() {
            problems.push("info.title: required and empty".to_owned());
        }
        if self.info.version.trim().is_empty() {
            problems.push("info.version: required and empty".to_owned());
        }
        if let Some(license) = &self.info.license {
            if license.identifier.is_some() && license.url.is_some() {
                problems.push(
                    "info.license: `identifier` and `url` are mutually exclusive in 3.1".to_owned(),
                );
            }
        }

        let mut seen_paths: Vec<&str> = Vec::new();
        let mut operation_ids: Vec<(String, String)> = Vec::new();
        let mut references: Vec<(String, String)> = Vec::new();

        for (path, item) in &self.paths {
            if !path.starts_with('/') {
                problems.push(format!("paths: `{path}` does not start with `/`"));
            }
            if seen_paths.contains(&path.as_str()) {
                problems.push(format!("paths: `{path}` appears twice"));
            }
            seen_paths.push(path);

            let template_vars = match path_variables(path) {
                Ok(vars) => vars,
                Err(message) => {
                    problems.push(format!("paths: `{path}`: {message}"));
                    Vec::new()
                }
            };

            for (verb, operation) in &item.operations {
                let at = format!("paths.{path}.{}", verb.as_str().to_lowercase());
                self.check_operation(
                    &at,
                    &template_vars,
                    item,
                    operation,
                    &mut problems,
                    &mut references,
                );
                if let Some((_, first)) = operation_ids
                    .iter()
                    .find(|(id, _)| *id == operation.operation_id)
                {
                    problems.push(format!(
                        "{at}: the operationId `{}` is already used by {first}; \
                         operationId must be unique across the document",
                        operation.operation_id
                    ));
                } else {
                    operation_ids.push((operation.operation_id.clone(), at.clone()));
                }
                if !is_operation_id(&operation.operation_id) {
                    problems.push(format!(
                        "{at}: the operationId `{}` has characters a generator cannot turn into \
                         a method name",
                        operation.operation_id
                    ));
                }
            }
        }

        for name in self.components.schema_names() {
            let schema = self
                .components
                .get_schema(name)
                .expect("name came from the map");
            schema.problems(&format!("components.schemas.{name}"), &mut problems);
            let mut found = Vec::new();
            schema.collect_references(&mut found);
            for reference in found {
                references.push((format!("components.schemas.{name}"), reference));
            }
        }

        for (at, name) in &references {
            if self.components.get_schema(name).is_none() {
                problems.push(format!(
                    "{at}: `$ref` to `#/components/schemas/{name}`, which is not defined"
                ));
            }
        }

        for requirement in &self.security {
            if !self.components.has_security_scheme(&requirement.scheme) {
                problems.push(format!(
                    "security: `{}` is not defined in components.securitySchemes",
                    requirement.scheme
                ));
            }
        }

        let mut tag_names: Vec<&str> = Vec::new();
        for tag in &self.tags {
            if tag_names.contains(&tag.name.as_str()) {
                problems.push(format!("tags: `{}` appears twice", tag.name));
            }
            tag_names.push(&tag.name);
        }

        if problems.is_empty() {
            Ok(())
        } else {
            Err(Validation { problems })
        }
    }

    fn check_operation(
        &self,
        at: &str,
        template_vars: &[String],
        item: &PathItem,
        operation: &Operation,
        problems: &mut Vec<String>,
        references: &mut Vec<(String, String)>,
    ) {
        let parameters: Vec<&Parameter> = item
            .parameters
            .iter()
            .chain(operation.parameters.iter())
            .collect();

        let mut seen: Vec<(&str, ParamIn)> = Vec::new();
        for parameter in &parameters {
            let key = (parameter.name.as_str(), parameter.location);
            if seen.contains(&key) {
                problems.push(format!(
                    "{at}: the parameter `{}` in `{}` is declared twice",
                    parameter.name,
                    parameter.location.as_str()
                ));
            }
            seen.push(key);
            if parameter.location == ParamIn::Path {
                if !parameter.required {
                    problems.push(format!(
                        "{at}: the path parameter `{}` is not marked required; a path parameter \
                         always is",
                        parameter.name
                    ));
                }
                if !template_vars.contains(&parameter.name) {
                    problems.push(format!(
                        "{at}: the path parameter `{}` does not appear in the path template",
                        parameter.name
                    ));
                }
            }
            parameter
                .schema
                .problems(&format!("{at}.parameters.{}", parameter.name), problems);
            let mut found = Vec::new();
            parameter.schema.collect_references(&mut found);
            for reference in found {
                references.push((format!("{at}.parameters.{}", parameter.name), reference));
            }
        }

        for name in template_vars {
            let declared = parameters
                .iter()
                .any(|p| p.location == ParamIn::Path && &p.name == name);
            if !declared {
                problems.push(format!(
                    "{at}: the path template has `{{{name}}}`, which is not in `parameters`; \
                     the document is invalid without it"
                ));
            }
        }

        if operation.responses.is_empty() {
            problems.push(format!(
                "{at}.responses: an operation must describe at least one response"
            ));
        }
        let mut statuses: Vec<&str> = Vec::new();
        for (status, response) in &operation.responses {
            if !is_status_key(status) {
                problems.push(format!(
                    "{at}.responses: `{status}` is neither `default` nor a status code"
                ));
            }
            if statuses.contains(&status.as_str()) {
                problems.push(format!("{at}.responses: `{status}` appears twice"));
            }
            statuses.push(status);
            if response.description.trim().is_empty() {
                problems.push(format!(
                    "{at}.responses.{status}: `description` is required and empty"
                ));
            }
            for (media_type, body) in &response.content {
                let where_at = format!("{at}.responses.{status}.content.{media_type}");
                body.schema.problems(&where_at, problems);
                let mut found = Vec::new();
                body.schema.collect_references(&mut found);
                for reference in found {
                    references.push((where_at.clone(), reference));
                }
            }
        }

        if let Some(body) = &operation.request_body {
            if body.content.is_empty() {
                problems.push(format!("{at}.requestBody: `content` is required and empty"));
            }
            for (media_type, media) in &body.content {
                let where_at = format!("{at}.requestBody.content.{media_type}");
                media.schema.problems(&where_at, problems);
                let mut found = Vec::new();
                media.schema.collect_references(&mut found);
                for reference in found {
                    references.push((where_at.clone(), reference));
                }
            }
        }

        if let Some(security) = &operation.security {
            for requirement in security {
                if !self.components.has_security_scheme(&requirement.scheme) {
                    problems.push(format!(
                        "{at}.security: `{}` is not defined in components.securitySchemes",
                        requirement.scheme
                    ));
                }
            }
        }
    }
}

/// The variable names in an OpenAPI path template.
///
/// # Errors
///
/// A message when a brace is unbalanced or a variable name is empty.
pub(crate) fn path_variables(path: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err("an unbalanced `{` in the path template".to_owned());
        };
        let name = &after[..close];
        if name.is_empty() {
            return Err("an empty path variable name".to_owned());
        }
        if name.contains('{') {
            return Err("a nested `{` in the path template".to_owned());
        }
        if names.contains(&name.to_owned()) {
            return Err(format!("the path variable `{name}` appears twice"));
        }
        names.push(name.to_owned());
        rest = &after[close + 1..];
    }
    if rest.contains('}') {
        return Err("an unbalanced `}` in the path template".to_owned());
    }
    Ok(names)
}

fn is_operation_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn is_status_key(key: &str) -> bool {
    if key == "default" {
        return true;
    }
    let bytes = key.as_bytes();
    if bytes.len() != 3 {
        return false;
    }
    if !(b'1'..=b'5').contains(&bytes[0]) {
        return false;
    }
    (bytes[1].is_ascii_digit() && bytes[2].is_ascii_digit())
        || (bytes[1] == b'X' && bytes[2] == b'X')
}

/// Everything structurally wrong with a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    /// One line per problem, in document order.
    pub problems: Vec<String>,
}

impl std::fmt::Display for Validation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "the OpenAPI document has {} structural {}:",
            self.problems.len(),
            if self.problems.len() == 1 {
                "problem"
            } else {
                "problems"
            }
        )?;
        for problem in &self.problems {
            writeln!(f, "  - {problem}")?;
        }
        Ok(())
    }
}

impl std::error::Error for Validation {}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal() -> Document {
        let mut doc = Document::new("Test", "1.0.0");
        let mut item = PathItem::default();
        item.operations
            .push((Verb::Get, Operation::new("get_health")));
        doc.paths.push(("/health".to_owned(), item));
        doc
    }

    #[test]
    fn a_minimal_document_validates() {
        minimal().validate().unwrap();
    }

    #[test]
    fn path_variables_are_extracted() {
        assert_eq!(
            path_variables("/users/{id}/posts/{post_id}").unwrap(),
            vec!["id".to_string(), "post_id".to_string()]
        );
        assert!(path_variables("/users/{id").is_err());
        assert!(path_variables("/users/{}").is_err());
    }

    #[test]
    fn a_missing_path_parameter_is_a_problem() {
        let mut doc = Document::new("Test", "1.0.0");
        let mut item = PathItem::default();
        item.operations
            .push((Verb::Get, Operation::new("get_user")));
        doc.paths.push(("/users/{id}".to_owned(), item));
        let error = doc.validate().unwrap_err();
        assert!(error.problems.iter().any(|p| p.contains("{id}")));
    }

    #[test]
    fn a_parameter_that_is_not_in_the_path_is_a_problem() {
        let mut doc = Document::new("Test", "1.0.0");
        let mut item = PathItem::default();
        let mut operation = Operation::new("get_user");
        operation
            .parameters
            .push(Parameter::path("ghost", Schema::string()));
        item.operations.push((Verb::Get, operation));
        doc.paths.push(("/users".to_owned(), item));
        let error = doc.validate().unwrap_err();
        assert!(error
            .problems
            .iter()
            .any(|p| p.contains("does not appear in the path template")));
    }

    #[test]
    fn a_dangling_ref_is_a_problem() {
        let mut doc = minimal();
        let mut item = PathItem::default();
        let mut operation = Operation::new("list_posts");
        operation.responses = vec![(
            "200".to_owned(),
            Response::json("ok", Schema::reference("Ghost")),
        )];
        item.operations.push((Verb::Get, operation));
        doc.paths.push(("/posts".to_owned(), item));
        let error = doc.validate().unwrap_err();
        assert!(error.problems.iter().any(|p| p.contains("Ghost")));
    }

    #[test]
    fn components_reject_a_name_collision_with_a_different_shape() {
        let mut components = Components::new();
        components.schema("Post", Schema::object()).unwrap();
        components.schema("Post", Schema::object()).unwrap();
        let error = components.schema("Post", Schema::string()).unwrap_err();
        assert_eq!(
            error,
            ComponentError::SchemaConflict {
                name: "Post".to_owned()
            }
        );
    }

    #[test]
    fn verbs_are_emitted_in_the_specification_order() {
        let mut item = PathItem::default();
        item.operations
            .push((Verb::Delete, Operation::new("destroy")));
        item.operations.push((Verb::Get, Operation::new("show")));
        let json = item.to_json();
        let keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        assert_eq!(keys, vec!["get", "delete"]);
    }
}
