//! The page-shaped surface: `mount`, for a generated project's one router.
//!
//! A generated project has one route table. `src/pages/mod.rs` defines what a
//! handler is:
//!
//! ```text
//! pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;
//! ```
//!
//! and every route source in that project offers one function:
//!
//! ```text
//! pub fn mount(
//!     routes: &mut brocanth::routing::RouterBuilder<Handler>,
//!     prefix: &str,
//! ) -> Result<(), brocanth::routing::BuildError>;
//! ```
//!
//! `Ctx`, `Response` and `Error` are the **project's** types, so this crate
//! cannot name them. It names their shape instead: [`PageCtx`], [`PageResponse`]
//! and [`PageError`] are the three things a records route needs from a page
//! triple, and a generated project writes one delegating line per method.
//! [`mount`] is then generic over the triple, and `RouterBuilder<Handler>` and
//! `RouterBuilder<Box<dyn Fn(&mut Ctx) -> …>>` are the same type, because a
//! type alias is transparent.
//!
//! The project's own `mount` is four lines:
//!
//! ```ignore
//! pub fn mount(routes: &mut RouterBuilder<Handler>, prefix: &str) -> Result<(), BuildError> {
//!     brocanth_resource::pages::mount(routes, prefix, Arc::new(Posts::new()))
//! }
//! ```
//!
//! # Why this is a second surface and not an adapter over the first
//!
//! [`routes`](crate::routes) contributes the same five operations as a
//! `RouteSource`, which is the core's bundle seam. Nothing outside the core
//! can drive that seam: `RouteBundle::new` is `pub(crate)`
//! (`brocanth/src/plugin_routes.rs:274`), so the only public way to call
//! `RouteSource::routes` is `App::mount_routes`, and a `RouteCx` then needs a
//! `Context<E>` that a generated `Ctx` keeps in a private field and never
//! lends out (`brocanth-cli/src/commands/create.rs:2253`). A general adapter
//! belongs in the core, where both seams and every host can see it. This
//! module is the part that does not: one resource, two shapes, one [`flow`].
//!
//! # The seven routes
//!
//! | name | verb | path | |
//! |---|---|---|---|
//! | list | `GET` | `/` | |
//! | create | `POST` | `/` | |
//! | view | `GET` | `/{id}` | |
//! | update | `PATCH` | `/{id}` | |
//! | delete | `DELETE` | `/{id}` | |
//! | update | `POST` | `/{id}` | a form |
//! | delete | `POST` | `/{id}/delete` | a form |
//!
//! The last two are the difference between this surface and the JSON one, and
//! the reason is the seam itself: these routes go in the table a **browser**
//! reaches, and an HTML form sends `GET` or `POST` and nothing else. Without
//! them a generated project can read a record from a page and cannot write
//! one, which is the same "written and unreachable" the crate already had, one
//! level down. They run the same [`flow`] functions, answer with the same
//! status table, and a project that wants the five REST routes alone calls
//! [`mount_api`] instead.
//!
//! # One table, so a path declared twice is a build error
//!
//! [`mount`] writes into the builder the project's pages write into.
//! A second mount at a prefix that is already taken is
//! [`BuildError::DuplicateHandler`](brocanth::routing::BuildError::DuplicateHandler),
//! from the router, naming the template and the verb. This module adds no
//! conflict rule of its own, because the table already has one.

use std::sync::Arc;

use brocanth::routing::{BuildError, RouterBuilder, Verb};

use crate::error::Failure;
use crate::flow;
use crate::render::{self, Answer};
use crate::resource::Resource;
use crate::status::Action;

// ---------------------------------------------------------------------------
// The three shapes of a page triple
// ---------------------------------------------------------------------------

/// What a records route reads out of a generated project's `Ctx`.
///
/// Four methods, and a generated `Ctx` already has all four. There is no
/// `verb` and no `path`: the router matched them, so a handler that read them
/// back would be asking which function it is in.
pub trait PageCtx {
    /// Everything after the first `?`, without it. Empty when there was none.
    fn query(&self) -> &str;

    /// The body as it arrived.
    fn body(&self) -> &[u8];

    /// A route parameter, or `None` for a name the route does not declare.
    fn param(&self, name: &str) -> Option<&str>;

    /// A request header by name, case-insensitively, or `None`.
    fn header(&self, name: &str) -> Option<&str>;
}

/// How a generated project builds a response.
///
/// One constructor, because a records route answers one way: a status, a body
/// and the headers that body needs. The project's own `Response` is a struct
/// with those three fields, so the impl is a struct literal.
pub trait PageResponse: Sized {
    /// A response with this status, this body and these headers.
    ///
    /// The body is always UTF-8: this crate writes JSON, and
    /// [`Resource::encode_row`] writes into the same buffer through
    /// [`crate::json`].
    fn page_response(status: u16, body: String, headers: Vec<(String, String)>) -> Self;
}

/// How a generated project reports a failure of its own machinery.
///
/// Not a refusal. A policy that refuses is an **answer** and is rendered with
/// the status [`crate::status`] gives it. This is a database that is down or a
/// policy that does not compile, and it goes wherever the project sends a 500
/// — which is the project's decision and not this crate's.
pub trait PageError: Sized {
    /// The project's error, with the status and the message to log.
    fn page_error(status: u16, message: String) -> Self;
}

/// How a resource reads the subject out of a page context.
///
/// The page-shaped twin of
/// [`SubjectSource`](crate::routes::SubjectSource), and generic over the
/// context type for the same reason that one is generic over the provider
/// list: a resource written for one project can read that project's own `Ctx`,
/// and a resource written for several is generic too.
///
/// This crate never learns what a subject is. PLAN.md §8 forbids assuming an
/// authentication scheme, and an anonymous visitor is the application's own
/// `None`, turned into a decision by `Bound::or_deny` where the policy reads
/// it.
pub trait PageSubject<C>: Resource {
    /// Who is asking.
    ///
    /// # Errors
    ///
    /// [`Failure::NoSubject`] when the request carries nothing this resource
    /// can read, which is a `401`.
    fn subject(&self, ctx: &C) -> Result<Self::Subject, Failure>;
}

/// A handler, as a generated project's table stores it.
///
/// The same type as the project's own `Handler` alias whenever `C`, `Resp` and
/// `Err` are its `Ctx`, `Response` and `Error`.
pub type PageHandler<C, Resp, Err> = Box<dyn Fn(&mut C) -> Result<Resp, Err> + Send + Sync>;

// ---------------------------------------------------------------------------
// Mounting
// ---------------------------------------------------------------------------

/// The seven routes, under a prefix this crate does not choose.
///
/// The five REST routes plus the two a browser form can reach. See the module
/// documentation for the table and the argument.
///
/// # Errors
///
/// [`BuildError::DuplicateHandler`] when the table already answers one of
/// these paths with one of these verbs, and
/// [`BuildError::Template`](brocanth::routing::BuildError::Template) for a
/// prefix that is not a path.
pub fn mount<R, C, Resp, Err>(
    routes: &mut RouterBuilder<PageHandler<C, Resp, Err>>,
    prefix: &str,
    resource: Arc<R>,
) -> Result<(), BuildError>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    mount_api(routes, prefix, Arc::clone(&resource))?;
    let one = join(prefix, "/{id}");

    // A form sends POST or it sends nothing. These two are the same two
    // operations, reached the only way a browser can reach them.
    routes.route(&one, Verb::Post, update_handler(Arc::clone(&resource)))?;
    routes.route(
        &join(prefix, "/{id}/delete"),
        Verb::Post,
        delete_handler(resource),
    )?;
    Ok(())
}

/// The five REST routes, and nothing a form needs.
///
/// For a project that mounts this resource under `/api` and writes its own
/// pages for the forms.
///
/// # Errors
///
/// As [`mount`].
pub fn mount_api<R, C, Resp, Err>(
    routes: &mut RouterBuilder<PageHandler<C, Resp, Err>>,
    prefix: &str,
    resource: Arc<R>,
) -> Result<(), BuildError>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    let collection = join(prefix, "/");
    let one = join(prefix, "/{id}");

    routes.route(&collection, Verb::Get, list_handler(Arc::clone(&resource)))?;
    routes.route(
        &collection,
        Verb::Post,
        create_handler(Arc::clone(&resource), collection.clone()),
    )?;
    routes.route(&one, Verb::Get, view_handler(Arc::clone(&resource)))?;
    routes.route(&one, Verb::Patch, update_handler(Arc::clone(&resource)))?;
    routes.route(&one, Verb::Delete, delete_handler(resource))?;
    Ok(())
}

/// Hangs `path` under `prefix`, without ever producing `//` or a trailing
/// slash that was not asked for.
///
/// The core joins a bundle's paths the same way, and its own `join_path` is
/// `pub(crate)`.
fn join(prefix: &str, path: &str) -> String {
    let prefix = prefix.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    if path.is_empty() {
        if prefix.is_empty() {
            return "/".to_string();
        }
        return prefix.to_string();
    }
    format!("{prefix}/{path}")
}

// ---------------------------------------------------------------------------
// The handlers
// ---------------------------------------------------------------------------
//
// Each one reads the context, drives `flow`, and hands what came back to
// `render`. They are the handlers of `crate::routes` with one difference: what
// a request is read out of. The status table is read in one place.

fn list_handler<R, C, Resp, Err>(resource: Arc<R>) -> PageHandler<C, Resp, Err>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    Box::new(move |ctx| {
        let answer = (|| {
            let subject = resource.subject(ctx)?;
            let query = query_of(ctx);
            let request = render::page_request(query)?;
            let sort = render::sort_from(&*resource, query)?;
            flow::list(&*resource, &subject, &request, sort)
        })();
        sent(match answer {
            Ok(listing) => render::envelope(&*resource, &listing),
            Err(failure) => render::refuse(Action::List, failure),
        })
    })
}

fn view_handler<R, C, Resp, Err>(resource: Arc<R>) -> PageHandler<C, Resp, Err>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    Box::new(move |ctx| {
        let answer = (|| {
            let subject = resource.subject(ctx)?;
            let id = require_id(ctx)?;
            flow::view(&*resource, &subject, id)
        })();
        sent(match answer {
            Ok(row) => render::one_row(&*resource, &row, 200, None),
            Err(failure) => render::refuse(Action::View, failure),
        })
    })
}

fn create_handler<R, C, Resp, Err>(resource: Arc<R>, prefix: String) -> PageHandler<C, Resp, Err>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    Box::new(move |ctx| {
        let answer = (|| {
            let subject = resource.subject(ctx)?;
            flow::create(&*resource, &subject, ctx.body())
        })();
        sent(match answer {
            // 201 and a `Location`, because a success code carries no
            // information to hide and HTTP already has an answer for this one.
            Ok(created) => {
                let location = format!(
                    "{}/{}",
                    prefix.trim_end_matches('/'),
                    render::display(&created.id)
                );
                render::one_row(&*resource, &created.row, 201, Some(location))
            }
            Err(failure) => render::refuse(Action::Create, failure),
        })
    })
}

fn update_handler<R, C, Resp, Err>(resource: Arc<R>) -> PageHandler<C, Resp, Err>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    Box::new(move |ctx| {
        let answer = (|| {
            let subject = resource.subject(ctx)?;
            let id = require_id(ctx)?.to_owned();
            flow::update(&*resource, &subject, &id, ctx.body())
        })();
        sent(match answer {
            Ok(updated) => match &updated.row {
                Some(row) => render::one_row(&*resource, row, 200, None),
                // The write stood; the row is no longer this subject's to see.
                None => render::empty(204),
            },
            Err(failure) => render::refuse(Action::Update, failure),
        })
    })
}

fn delete_handler<R, C, Resp, Err>(resource: Arc<R>) -> PageHandler<C, Resp, Err>
where
    R: PageSubject<C>,
    C: PageCtx + 'static,
    Resp: PageResponse + 'static,
    Err: PageError + 'static,
{
    Box::new(move |ctx| {
        let answer = (|| {
            let subject = resource.subject(ctx)?;
            let id = require_id(ctx)?;
            flow::delete(&*resource, &subject, id)
        })();
        sent(match answer {
            Ok(()) => render::empty(204),
            Err(failure) => render::refuse(Action::Delete, failure),
        })
    })
}

// ---------------------------------------------------------------------------
// Reading a request, and answering it
// ---------------------------------------------------------------------------

/// The query string, or `None` when there was none.
///
/// A generated `Ctx` holds `""` for both "no `?`" and "`?` and nothing after
/// it". [`render::page_request`] wants the `Option` the core's `Uri` gives it,
/// and the two spell the same absence.
fn query_of<C: PageCtx>(ctx: &C) -> Option<&str> {
    let query = ctx.query();
    if query.is_empty() {
        None
    } else {
        Some(query)
    }
}

/// The `{id}` segment the route declares.
///
/// Missing is this crate disagreeing with the path it mounted itself at, which
/// is a `400` about a request nobody could have sent correctly. The core's
/// surface answers the same way.
fn require_id<C: PageCtx>(ctx: &C) -> Result<&str, Failure> {
    ctx.param("id")
        .ok_or_else(|| Failure::bad_request("the route declares no `id` parameter"))
}

/// An [`Answer`] in the project's own types.
///
/// The one place this module decides anything: an internal failure becomes the
/// project's `Error`, which is where a 500 goes, and everything else is a
/// response.
fn sent<Resp: PageResponse, Err: PageError>(answer: Answer) -> Result<Resp, Err> {
    match answer {
        Answer::Json {
            status,
            body,
            location,
        } => {
            let mut headers = vec![("content-type".to_owned(), "application/json".to_owned())];
            if let Some(location) = location {
                headers.push(("location".to_owned(), location));
            }
            match String::from_utf8(body) {
                Ok(body) => Ok(Resp::page_response(status, body, headers)),
                // `encode_row` wrote bytes that are not text. A page response
                // carries a `String`, so there is nothing honest to send.
                Err(error) => Err(Err::page_error(
                    500,
                    format!("this resource encoded a row that is not UTF-8: {error}"),
                )),
            }
        }
        // No body, so no `content-type`: there is nothing for one to describe.
        Answer::Empty { status } => Ok(Resp::page_response(status, String::new(), Vec::new())),
        Answer::Internal(failure) => Err(Err::page_error(500, failure.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::join;

    #[test]
    fn a_prefix_and_a_path_join_without_a_double_slash() {
        assert_eq!(join("/posts", "/"), "/posts");
        assert_eq!(join("/posts/", "/"), "/posts");
        assert_eq!(join("/posts", "/{id}"), "/posts/{id}");
        assert_eq!(join("/", "/"), "/");
        assert_eq!(join("/", "/{id}"), "/{id}");
    }
}
