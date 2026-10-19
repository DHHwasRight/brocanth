//! The route bundle, the status codes and the JSON envelope.
//!
//! ADR 3: *"a plugin describes a bundle of routes relative to a prefix it does
//! not choose"*. So this crate contributes five routes under `"/"` and the
//! application writes one line:
//!
//! ```text
//! app.mount_routes::<ResourceRoutes<Posts>>("/api/collections/posts/records")?;
//! ```
//!
//! | name | verb | path |
//! |---|---|---|
//! | `<name>.list` | `GET` | `/` |
//! | `<name>.create` | `POST` | `/` |
//! | `<name>.view` | `GET` | `/{id}` |
//! | `<name>.update` | `PATCH` | `/{id}` |
//! | `<name>.delete` | `DELETE` | `/{id}` |
//!
//! `<name>` is the plugin instance name, so two resources in one application
//! are `app.plugin_as("posts", …)` and `app.plugin_as("comments", …)` and
//! their route names do not collide. A path that two bundles both claim is
//! `RouteError::Conflict` naming both, which is the core's rule and not this
//! crate's business.
//!
//! # The query string
//!
//! `page`, `perPage`, `skipTotal` and `sort`, spelled as
//! `docs/pocketbase-surface.md` §2 spells them. Values are read literally:
//! they are numbers, a flag and a column name, none of which needs
//! percent-decoding, and a decoder here would be a parser this crate does not
//! need. `filter`, `expand` and `fields` are not accepted — §9 is the argument
//! against the first and §12 declines the shape the other two serve.
//!
//! A `sort` name this resource does not map is a `400`. Ignoring it would
//! silently serve a different order than the one that was asked for, and an
//! ordering that is not the one the client paginated against is the paginator
//! lying again.
//!
//! # Errors leave by two different doors
//!
//! A refusal is an **answer**: it is rendered here, with the status the table
//! in [`crate::status`] gives. A failure of the application's own machinery —
//! a database that is down, a policy that does not compile — leaves as a
//! `HandlerError`, so the application's error hooks see it and a logger can
//! log it. [`crate::Failure::is_internal`] is the line between them.

use std::sync::Arc;

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{HandlerError, Response};
use brocanth::plugin_routes::{Handler, RouteBundle, RouteCx, RouteSource};
use brocanth_policy::Value;

use crate::error::Failure;
use crate::flow;
use crate::json;
use crate::page::{Listing, PageRequest};
use crate::resource::{ColumnOf, Direction, Resource, RowOf, Sort};
use crate::status::{generic_message, outcome, Action};

/// How a resource reads the subject out of a request.
///
/// Separate from [`Resource`] because it is the one thing that depends on the
/// application's context provider list `E`, exactly as
/// [`RouteSource<E>`](brocanth::plugin_routes::RouteSource) does. A resource
/// written against one application can use `Context::get` and have the
/// provider checked at compile time; a resource written for several uses
/// `Context::try_get`.
///
/// This crate never learns what a subject is. PLAN.md §8 forbids assuming an
/// authentication scheme, and `brocanth-policy` says the same: an anonymous
/// request is the application's own `None`, turned into a decision by
/// `Bound::or_deny` at the point the policy reads it.
pub trait SubjectSource<E>: Resource {
    /// Who is asking.
    ///
    /// # Errors
    ///
    /// [`Failure::NoSubject`] when the request carries nothing this resource
    /// can read, which is a `401`. An **anonymous** visitor is not an error:
    /// it is a subject with a `None` inside, and the policy decides what that
    /// means.
    fn subject(&self, cx: &RouteCx<'_, E>) -> Result<Self::Subject, Failure>;
}

/// The plugin that offers one resource's five routes.
///
/// Holds the resource in an `Arc` because every handler is a `'static`
/// closure and all five share one resource.
pub struct ResourceRoutes<R> {
    resource: Arc<R>,
}

impl<R: Resource> ResourceRoutes<R> {
    /// The resource, for an application that wants to reach it after
    /// registering.
    pub fn resource(&self) -> &Arc<R> {
        &self.resource
    }
}

impl<R: Resource> Plugin for ResourceRoutes<R> {
    type Config = R;
    type Provides = NoContext;
    const NAME: &'static str = R::NAME;
    const CONTRIBUTES_ROUTES: bool = true;

    fn new(config: Self::Config) -> Self {
        ResourceRoutes {
            resource: Arc::new(config),
        }
    }
}

impl<E: 'static, R: SubjectSource<E>> RouteSource<E> for ResourceRoutes<R> {
    const PREFIX: &'static str = R::PREFIX;

    fn routes(&self, bundle: &mut RouteBundle<'_, E>) -> Result<(), PluginError> {
        let prefix = bundle.prefix().to_owned();
        bundle.described(
            "list",
            brocanth::routing_verb::Verb::Get,
            "/",
            "one page of the rows this subject may see",
            list_handler(Arc::clone(&self.resource)),
        );
        bundle.described(
            "create",
            brocanth::routing_verb::Verb::Post,
            "/",
            "create a row, if the policy admits the one proposed",
            create_handler(Arc::clone(&self.resource), prefix),
        );
        bundle.described(
            "view",
            brocanth::routing_verb::Verb::Get,
            "/{id}",
            "one row, guarded by the view rule in the same statement",
            view_handler(Arc::clone(&self.resource)),
        );
        bundle.described(
            "update",
            brocanth::routing_verb::Verb::Patch,
            "/{id}",
            "change a row, through a witness that carries its condition",
            update_handler(Arc::clone(&self.resource)),
        );
        bundle.described(
            "delete",
            brocanth::routing_verb::Verb::Delete,
            "/{id}",
            "remove a row, through the same witness",
            delete_handler(Arc::clone(&self.resource)),
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// The five handlers
// ---------------------------------------------------------------------------

fn list_handler<E: 'static, R: SubjectSource<E>>(resource: Arc<R>) -> Handler<E> {
    Box::new(move |cx| {
        let answer = (|| {
            let subject = resource.subject(cx)?;
            let query = cx.request().uri().query();
            let request = page_request(query)?;
            let sort = sort_from(&*resource, query)?;
            flow::list(&*resource, &subject, &request, sort)
        })();
        match answer {
            Ok(listing) => envelope(&*resource, &listing),
            Err(failure) => refuse(Action::List, failure),
        }
    })
}

fn view_handler<E: 'static, R: SubjectSource<E>>(resource: Arc<R>) -> Handler<E> {
    Box::new(move |cx| {
        let answer = (|| {
            let subject = resource.subject(cx)?;
            let id = cx
                .params()
                .require("id")
                .map_err(|error| Failure::bad_request(error.to_string()))?;
            flow::view(&*resource, &subject, id)
        })();
        match answer {
            Ok(row) => one_row(&*resource, &row, 200, None),
            Err(failure) => refuse(Action::View, failure),
        }
    })
}

fn create_handler<E: 'static, R: SubjectSource<E>>(resource: Arc<R>, prefix: String) -> Handler<E> {
    Box::new(move |cx| {
        let answer = (|| {
            let subject = resource.subject(cx)?;
            flow::create(&*resource, &subject, cx.request().body())
        })();
        match answer {
            // 201 and a `Location`, because a success code carries no
            // information to hide and HTTP already has an answer for this one.
            Ok(created) => {
                let location = format!("{}/{}", prefix.trim_end_matches('/'), display(&created.id));
                one_row(&*resource, &created.row, 201, Some(location))
            }
            Err(failure) => refuse(Action::Create, failure),
        }
    })
}

fn update_handler<E: 'static, R: SubjectSource<E>>(resource: Arc<R>) -> Handler<E> {
    Box::new(move |cx| {
        let answer = (|| {
            let subject = resource.subject(cx)?;
            let id = cx
                .params()
                .require("id")
                .map_err(|error| Failure::bad_request(error.to_string()))?
                .to_owned();
            flow::update(&*resource, &subject, &id, cx.request().body())
        })();
        match answer {
            Ok(updated) => match &updated.row {
                Some(row) => one_row(&*resource, row, 200, None),
                // The write stood; the row is no longer this subject's to see.
                None => Ok(empty(204)),
            },
            Err(failure) => refuse(Action::Update, failure),
        }
    })
}

fn delete_handler<E: 'static, R: SubjectSource<E>>(resource: Arc<R>) -> Handler<E> {
    Box::new(move |cx| {
        let answer = (|| {
            let subject = resource.subject(cx)?;
            let id = cx
                .params()
                .require("id")
                .map_err(|error| Failure::bad_request(error.to_string()))?;
            flow::delete(&*resource, &subject, id)
        })();
        match answer {
            Ok(()) => Ok(empty(204)),
            Err(failure) => refuse(Action::Delete, failure),
        }
    })
}

// ---------------------------------------------------------------------------
// The query string
// ---------------------------------------------------------------------------

/// `page`, `perPage` and `skipTotal`.
///
/// A number that is not a number is a `400`. Clamping it to a default would
/// serve page one to somebody who asked for page `two`, which is the kind of
/// quiet repair that makes a paginator untrustworthy.
pub fn page_request(query: Option<&str>) -> Result<PageRequest, Failure> {
    let mut request = PageRequest::new();
    if let Some(raw) = param(query, "page") {
        request.page = raw
            .parse()
            .map_err(|_| Failure::bad_request(format!("`page` is not a number: `{raw}`")))?;
    }
    if let Some(raw) = param(query, "perPage") {
        request.per_page =
            Some(raw.parse().map_err(|_| {
                Failure::bad_request(format!("`perPage` is not a number: `{raw}`"))
            })?);
    }
    if let Some(raw) = param(query, "skipTotal") {
        request.skip_total = matches!(raw, "" | "1" | "true");
    }
    Ok(request)
}

/// `sort=-created_at`, mapped through [`Resource::sort_column`].
fn sort_from<R: Resource>(
    resource: &R,
    query: Option<&str>,
) -> Result<Option<Sort<ColumnOf<R>>>, Failure> {
    let Some(raw) = param(query, "sort") else {
        return Ok(None);
    };
    let (direction, name) = match raw.strip_prefix('-') {
        Some(rest) => (Direction::Descending, rest),
        None => (Direction::Ascending, raw.trim_start_matches('+')),
    };
    match resource.sort_column(name) {
        Some(column) => Ok(Some(Sort { column, direction })),
        None => Err(Failure::bad_request(format!(
            "`{name}` is not a sortable column of this resource"
        ))),
    }
}

fn param<'a>(query: Option<&'a str>, name: &str) -> Option<&'a str> {
    query?
        .split('&')
        .find_map(|pair| match pair.split_once('=') {
            Some((key, value)) if key == name => Some(value),
            None if pair == name => Some(""),
            _ => None,
        })
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// `{"page":1,"perPage":30,"totalItems":2,"totalPages":1,"items":[…]}`.
///
/// `totalItems` and `totalPages` are **absent** when the client passed
/// `skipTotal`, rather than present and wrong. See [`crate::page`].
fn envelope<R: Resource>(
    resource: &R,
    listing: &Listing<RowOf<R>>,
) -> Result<Response, HandlerError> {
    let mut body = Vec::with_capacity(64 + listing.items.len() * 64);
    body.push(b'{');
    json::push_key(&mut body, "page");
    json::push_u64(&mut body, listing.page);
    body.push(b',');
    json::push_key(&mut body, "perPage");
    json::push_u64(&mut body, listing.per_page);
    if let Some(total) = listing.total.count() {
        body.push(b',');
        json::push_key(&mut body, "totalItems");
        json::push_u64(&mut body, total);
        if let Some(pages) = listing.total.pages(listing.per_page) {
            body.push(b',');
            json::push_key(&mut body, "totalPages");
            json::push_u64(&mut body, pages);
        }
    }
    body.push(b',');
    json::push_key(&mut body, "items");
    body.push(b'[');
    for (index, row) in listing.items.iter().enumerate() {
        if index > 0 {
            body.push(b',');
        }
        if let Err(failure) = resource.encode_row(row, &mut body) {
            return refuse(Action::List, failure);
        }
    }
    body.extend_from_slice(b"]}");
    Ok(json_response(200, body, None))
}

fn one_row<R: Resource>(
    resource: &R,
    row: &RowOf<R>,
    status: u16,
    location: Option<String>,
) -> Result<Response, HandlerError> {
    let mut body = Vec::with_capacity(128);
    if let Err(failure) = resource.encode_row(row, &mut body) {
        return refuse(Action::View, failure);
    }
    Ok(json_response(status, body, location))
}

/// A refusal, rendered; or an internal failure, handed to the core.
fn refuse(action: Action, failure: Failure) -> Result<Response, HandlerError> {
    let answer = outcome(action, &failure);
    if answer.internal {
        // Out through the other door, so the application's error hooks see it.
        return Err(HandlerError::new(failure));
    }
    // A list that nothing matches is an empty page, not an error body: an
    // error body here would be the oracle the 200 exists to avoid.
    if action == Action::List && answer.status == 200 {
        return Ok(json_response(
            200,
            b"{\"page\":1,\"perPage\":0,\"totalItems\":0,\"totalPages\":0,\"items\":[]}".to_vec(),
            None,
        ));
    }
    let message = if answer.explain {
        failure.to_string()
    } else {
        generic_message(answer.status).to_owned()
    };
    Ok(json_response(
        answer.status,
        json::error_body(answer.status, &message),
        None,
    ))
}

fn json_response(status: u16, body: Vec<u8>, location: Option<String>) -> Response {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() =
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/json"),
    );
    if let Some(location) = location {
        if let Ok(value) = http::HeaderValue::from_str(&location) {
            response.headers_mut().insert(http::header::LOCATION, value);
        }
    }
    response
}

fn empty(status: u16) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() =
        http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR);
    response
}

/// A key, as a `Location` header spells it.
fn display(id: &Value) -> String {
    match id {
        Value::Text(text) => text.clone(),
        other => other.to_string(),
    }
}
