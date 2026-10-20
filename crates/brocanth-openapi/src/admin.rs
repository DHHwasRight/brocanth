//! The document, as a tab in `brocanth-admin`.
//!
//! PocketBase's dashboard has an API preview generated from the collections.
//! This is the same screen, generated from the route table, and it needs no
//! read this crate did not already have: the document is built and validated
//! in [`Plugin::configure`](brocanth::plugin_api::Plugin::configure), and
//! [`document_of`](crate::document_of) hands it over.
//!
//! # Two screens
//!
//! **The list.** Every operation, ordered by path and then by the order the
//! verbs are declared in, with its verb, its path, its summary and its tags.
//! Sorted by path rather than grouped by tag, because the question an
//! operator asks this screen is "what is at `/posts/{id}`", and a tag is a
//! documentation choice the application may not have made at all.
//!
//! **One operation.** Its parameters, its request body and its responses,
//! each with the schema named rather than printed. A schema is a tree and
//! this is a preview; the name is the thing a reader can then search the
//! JSON for, and the JSON is one route away.
//!
//! # No try-it console
//!
//! The crate documentation refuses one and the reason holds twice over here:
//! an "Execute" button inside a backoffice is a request-forwarding endpoint
//! with an operator's cookies on it. This tab renders what the document
//! says, and sends nothing.
//!
//! # Read only
//!
//! A document is derived from the route table at startup. Nothing on this
//! screen could change it that would not be a lie by the next deploy, so
//! the panel ignores
//! [`PanelRequest::can_act`](brocanth_admin::panel::PanelRequest::can_act):
//! there is nothing to hide from a reader who may not act.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth::routing::Verb;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::view::render;

use crate::document::{Document, Operation, Parameter};
use crate::json::Json;
use crate::schema::Schema;

/// The panel's stylesheet, built from `style/panel.css`. Inlined by the
/// header, which both screens draw. See `style/README.md`.
pub(crate) const STYLE: &str = include_str!("admin.css");

/// The document as a tab.
///
/// ```ignore
/// AdminConfig::new().panel(ApiPanel::new(openapi.shared()))
/// ```
///
/// `openapi` is the [`OpenApiPlugin`](crate::OpenApiPlugin) the application
/// built, and [`shared`](crate::OpenApiPlugin::shared) is the handle that
/// outlives a borrow of it. A plugin that is handed
/// [`Services`](brocanth::plugin_api::Services) instead reaches the same
/// document through [`document_of`](crate::document_of).
pub struct ApiPanel {
    document: Arc<Document>,
    key: String,
    label: String,
}

impl ApiPanel {
    /// The panel, at `api`, labelled "API".
    pub fn new(document: Arc<Document>) -> Self {
        Self {
            document,
            key: "api".to_string(),
            label: "API".to_string(),
        }
    }

    /// A different key, for an application that already has an `api` model.
    #[must_use]
    pub fn at(mut self, key: impl Into<String>) -> Self {
        self.key = key.into();
        self
    }

    /// A different tab label.
    #[must_use]
    pub fn labelled(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    // ------------------------------------------------------------ screens

    fn operations(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let doc = &self.document;
        // Sorted before the rows are built, so the ordering key -- where
        // the verb comes in `Verb::ALL` -- never has to be carried into a
        // context the template would then have to ignore.
        let mut found: Vec<(&str, Verb, &Operation)> = doc
            .paths
            .iter()
            .flat_map(|(path, item)| {
                item.operations
                    .iter()
                    .map(move |(verb, op)| (path.as_str(), *verb, op))
            })
            .collect();
        found.sort_by_key(|(path, verb, _)| (*path, verb.index()));
        let rows: Vec<OperationRow> = found
            .iter()
            .map(|(path, verb, op)| row(path, *verb, op, r))
            .collect();

        let ctx = OperationsCtx {
            title: doc.info.title.clone(),
            version: doc.info.version.clone(),
            counted: counted(rows.len(), doc.paths.len()),
            servers: doc
                .servers
                .iter()
                .map(|s| match &s.description {
                    Some(text) => format!("{} — {text}", s.url),
                    None => s.url.clone(),
                })
                .collect(),
            rows,
        };
        let html = render(
            &|ctx: &OperationsCtx, w: &mut rsx::Writer<'_>| {
                use rsx::Template as _;
                crate::admin_templates::operations::Operations { ctx }.render(w)
            },
            &ctx,
        )?;
        Ok(PanelPage::body("API", html))
    }

    fn operation(&self, id: &str, r: &PanelRequest<'_>) -> Option<Result<PanelPage, AdminError>> {
        let (path, verb, op) = self.find(id)?;
        let ctx = OperationCtx {
            back_href: r.url(""),
            verb: verb.as_str().to_string(),
            path: path.to_string(),
            operation_id: op.operation_id.clone(),
            summary: op.summary.clone().unwrap_or_default(),
            description: op.description.clone().unwrap_or_default(),
            tags: op.tags.join(", "),
            deprecated: op.deprecated,
            security: security(op, &self.document),
            parameters: op.parameters.iter().map(param_row).collect(),
            request_body: op
                .request_body
                .iter()
                .flat_map(|body| {
                    body.content.iter().map(move |(media_type, media)| BodyRow {
                        media_type: media_type.clone(),
                        schema: type_name(&media.schema),
                        required: yes_no(body.required),
                    })
                })
                .collect(),
            responses: op
                .responses
                .iter()
                .map(|(status, response)| ResponseRow {
                    status: status.clone(),
                    description: response.description.clone(),
                    content: match response.content.is_empty() {
                        true => "no body".to_string(),
                        false => response
                            .content
                            .iter()
                            .map(|(media_type, media)| {
                                format!("{media_type} · {}", type_name(&media.schema))
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    },
                })
                .collect(),
        };
        let html = render(
            &|ctx: &OperationCtx, w: &mut rsx::Writer<'_>| {
                use rsx::Template as _;
                crate::admin_templates::operation::OperationPage { ctx }.render(w)
            },
            &ctx,
        );
        Some(
            html.map(|html| PanelPage::body(format!("{} {}", verb.as_str(), path), html))
                .map_err(AdminError::from),
        )
    }

    /// The operation with this id, wherever it is in the document.
    ///
    /// An `operationId` is unique across a valid document — the builder
    /// refuses a duplicate — so this is a lookup and not a search with a
    /// tie-break.
    fn find(&self, id: &str) -> Option<(&str, Verb, &Operation)> {
        self.document.paths.iter().find_map(|(path, item)| {
            item.operations
                .iter()
                .find(|(_, op)| op.operation_id == id)
                .map(|(verb, op)| (path.as_str(), *verb, op))
        })
    }
}

#[async_trait]
impl Panel for ApiPanel {
    fn key(&self) -> &str {
        &self.key
    }

    fn label(&self) -> String {
        self.label.clone()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        // A document is read-only, so a POST is not a screen this panel
        // serves and the admin turns it into a 404.
        if r.post {
            return Ok(None);
        }
        match r.path.split_once('/') {
            None if r.path.is_empty() => self.operations(&r).map(Some),
            Some(("op", id)) => self.operation(id, &r).transpose(),
            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Reading the document into what a screen prints
// ---------------------------------------------------------------------------

fn row(path: &str, verb: Verb, op: &Operation, r: &PanelRequest<'_>) -> OperationRow {
    OperationRow {
        verb: verb.as_str().to_string(),
        path: path.to_string(),
        summary: op.summary.clone().unwrap_or_default(),
        operation_id: op.operation_id.clone(),
        tags: op.tags.join(", "),
        deprecated: op.deprecated,
        href: r.url(&format!("op/{}", op.operation_id)),
    }
}

fn param_row(p: &Parameter) -> ParamRow {
    ParamRow {
        name: p.name.clone(),
        location: p.location.as_str().to_string(),
        required: yes_no(p.required),
        schema: type_name(&p.schema),
        description: p.description.clone().unwrap_or_default(),
    }
}

/// "12 operations over 7 paths", in whichever singular the numbers need.
fn counted(operations: usize, paths: usize) -> String {
    let ops = match operations {
        1 => "1 operation".to_string(),
        n => format!("{n} operations"),
    };
    let paths = match paths {
        1 => "1 path".to_string(),
        n => format!("{n} paths"),
    };
    format!("{ops} over {paths}")
}

/// What the document says about authentication for this operation.
///
/// An operation with no `security` of its own inherits the document's, and
/// an empty list is the OpenAPI spelling of "this one is public". Both of
/// those are invisible in the JSON unless you know the rule, which is
/// exactly why a preview should say them out loud.
fn security(op: &Operation, doc: &Document) -> String {
    let (requirements, inherited) = match &op.security {
        Some(own) => (own.as_slice(), false),
        None => (doc.security.as_slice(), true),
    };
    if requirements.is_empty() {
        return match inherited {
            true => "None. The document declares none either.".to_string(),
            false => "None. This operation is declared public.".to_string(),
        };
    }
    let named = requirements
        .iter()
        .map(|r| match r.scopes.is_empty() {
            true => r.scheme.clone(),
            false => format!("{} ({})", r.scheme, r.scopes.join(", ")),
        })
        .collect::<Vec<_>>()
        .join(" or ");
    match inherited {
        true => format!("{named} — inherited from the document"),
        false => named,
    }
}

fn yes_no(value: bool) -> String {
    match value {
        true => "required".to_string(),
        false => "optional".to_string(),
    }
}

/// A schema as one readable word.
///
/// Read off the rendered JSON rather than off the `Schema`, because
/// [`Schema`]'s fields are private and `to_json` is the shape this crate
/// has already committed to. A `$ref` answers with the component's name,
/// which is the string a reader can then search the document for.
fn type_name(schema: &Schema) -> String {
    name_of(&schema.to_json())
}

fn name_of(value: &Json) -> String {
    if let Some(reference) = value.get("$ref").and_then(Json::as_str) {
        return reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string();
    }
    if let Some(items) = value.get("items") {
        return format!("{}[]", name_of(items));
    }
    for keyword in ["oneOf", "anyOf", "allOf"] {
        if let Some(Json::Array(parts)) = value.get(keyword) {
            let joined = parts.iter().map(name_of).collect::<Vec<_>>().join(" | ");
            return match keyword {
                "allOf" => format!("all of {joined}"),
                _ => joined,
            };
        }
    }
    let base = match value.get("type") {
        Some(Json::Str(name)) => name.clone(),
        // `["string", "null"]` is how 3.1 spells a nullable string.
        Some(Json::Array(parts)) => {
            let mut names: Vec<&str> = parts.iter().filter_map(Json::as_str).collect();
            let nullable = names.contains(&"null");
            names.retain(|n| *n != "null");
            let joined = match names.is_empty() {
                true => "null".to_string(),
                false => names.join(" | "),
            };
            match nullable && !names.is_empty() {
                true => format!("{joined} or null"),
                false => joined,
            }
        }
        _ => "any".to_string(),
    };
    match value.get("format").and_then(Json::as_str) {
        Some(format) => format!("{base} ({format})"),
        None => base,
    }
}

// ---------------------------------------------------------------------------
// What the templates are given
// ---------------------------------------------------------------------------
//
// Every field is already a string the screen prints. No formatting happens
// in a template, which is what lets somebody replace one without
// reimplementing the sentences above.

/// One operation, as a row on the list.
pub struct OperationRow {
    /// `GET`.
    pub verb: String,
    /// `/posts/{id}`.
    pub path: String,
    /// What it does, or empty when the application did not say.
    pub summary: String,
    /// The `operationId`.
    pub operation_id: String,
    /// Its tags, comma separated.
    pub tags: String,
    /// Whether the document marks it deprecated.
    pub deprecated: bool,
    /// The detail screen.
    pub href: String,
}

/// The list.
pub struct OperationsCtx {
    /// The document's title.
    pub title: String,
    /// Its version.
    pub version: String,
    /// "12 operations over 7 paths".
    pub counted: String,
    /// The declared servers, each as one line.
    pub servers: Vec<String>,
    /// Every operation, ordered by path then by verb.
    pub rows: Vec<OperationRow>,
}

/// One parameter.
pub struct ParamRow {
    /// Its name.
    pub name: String,
    /// `path`, `query`, `header` or `cookie`.
    pub location: String,
    /// "required" or "optional".
    pub required: String,
    /// Its type, as one word.
    pub schema: String,
    /// What the document says about it.
    pub description: String,
}

/// One media type of a request body.
pub struct BodyRow {
    /// `application/json`.
    pub media_type: String,
    /// Its schema, as one word.
    pub schema: String,
    /// "required" or "optional".
    pub required: String,
}

/// One response.
pub struct ResponseRow {
    /// `200`, or `default`.
    pub status: String,
    /// What it means.
    pub description: String,
    /// Its media types and their schemas, or "no body".
    pub content: String,
}

/// One operation's screen.
pub struct OperationCtx {
    /// Back to the list.
    pub back_href: String,
    /// `GET`.
    pub verb: String,
    /// `/posts/{id}`.
    pub path: String,
    /// The `operationId`.
    pub operation_id: String,
    /// What it does, or empty.
    pub summary: String,
    /// The long form, or empty.
    pub description: String,
    /// Its tags, comma separated, or empty.
    pub tags: String,
    /// Whether the document marks it deprecated.
    pub deprecated: bool,
    /// What the document says about authentication, in a sentence.
    pub security: String,
    /// Its parameters.
    pub parameters: Vec<ParamRow>,
    /// Its request body, one row per media type.
    pub request_body: Vec<BodyRow>,
    /// Its responses.
    pub responses: Vec<ResponseRow>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::SecurityRequirement;

    #[test]
    fn a_schema_is_named_rather_than_printed() {
        assert_eq!(type_name(&Schema::string()), "string");
        assert_eq!(type_name(&Schema::string().nullable()), "string or null");
        assert_eq!(
            type_name(&Schema::string().format("date-time")),
            "string (date-time)"
        );
        assert_eq!(type_name(&Schema::reference("Post")), "Post");
        assert_eq!(
            type_name(&Schema::array(Schema::reference("Post"))),
            "Post[]"
        );
        assert_eq!(type_name(&Schema::any()), "any");
    }

    #[test]
    fn an_inherited_requirement_says_that_it_was_inherited() {
        let mut doc = Document::new("Blog", "1.0.0");
        doc.security = vec![SecurityRequirement::new("bearer")];
        let op = Operation::new("get_posts");
        assert_eq!(security(&op, &doc), "bearer — inherited from the document");
    }

    #[test]
    fn an_empty_list_on_the_operation_means_public_and_says_so() {
        let mut doc = Document::new("Blog", "1.0.0");
        doc.security = vec![SecurityRequirement::new("bearer")];
        let mut op = Operation::new("get_posts");
        op.security = Some(Vec::new());
        assert_eq!(
            security(&op, &doc),
            "None. This operation is declared public."
        );
    }

    #[test]
    fn the_count_is_said_in_the_right_singular() {
        assert_eq!(counted(1, 1), "1 operation over 1 path");
        assert_eq!(counted(12, 7), "12 operations over 7 paths");
    }
}
