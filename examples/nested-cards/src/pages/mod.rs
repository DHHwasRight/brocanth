//! The route table, which is this directory.
//!
//! One file is one route, and the path on disk is the URL: `index.rs` is `/`,
//! `about.rs` is `/about`, `users/[id].rs` is `/users/{id}`.
//!
//! Nothing below lists them. The macro reads this directory while the crate
//! is compiled and writes the `mod` lines and [`router`] from what is there,
//! so adding a page is adding a file and renaming one renames its route.
//! The verb comes off the handler's name: `handle` is GET, `handle_post` is
//! POST, and a `pub fn handle_*` that is not a verb is a build error rather
//! than a handler nothing reaches.
//!
//! `brocanth_pages::pages!` and `cargo expand` is how to read the result.

brocanth_pages::pages!("src/pages");

use brocanth::routing::{BuildError, Params, Router, Verb};

/// What a handler produces.
///
/// The framework owns rendering: a handler returns a value and never writes a
/// response itself, so a half-written body on an error is not something that
/// can happen. Echo and Gin hand the handler a writer and take an error back,
/// and both carry the scar -- once a 200 has gone out, the central error
/// handler cannot take it back. Here there is nothing to take back.
pub struct Response {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

/// Written by hand rather than derived, and the difference matters.
///
/// A derived `Default` would give `status: 0`, which is not an HTTP status, so
/// `Response { body: .., ..Default::default() }` would compile and be wrong.
/// 200 is what "I did not say" means for a response.
impl Default for Response {
    fn default() -> Self {
        Self {
            status: 200,
            body: String::new(),
            headers: Vec::new(),
        }
    }
}

impl Response {
    pub fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            ..Default::default()
        }
    }

    pub fn ok(body: impl Into<String>) -> Self {
        Self::new(200, body)
    }

    pub fn empty(status: u16) -> Self {
        Self::new(status, "")
    }

    /// For what the struct literal does not cover: several headers, added one
    /// at a time. Rust has no variadic generics, so a tuple return is
    /// fixed-arity and this is the unbounded form.
    #[must_use]
    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// What a handler may return instead of building a [`Response`] by hand.
///
/// The Crystal brocanth overloads `write_response` on the handler's return
/// type, so a page returns the simplest thing that says what it means. Rust
/// spells the same idea as a trait: each handler still returns exactly one
/// type, and this is what lets different handlers return different ones.
pub trait IntoResponse {
    fn into_response(self) -> Response;
}

impl IntoResponse for Response {
    fn into_response(self) -> Response {
        self
    }
}

/// A body, answered 200.
impl IntoResponse for String {
    fn into_response(self) -> Response {
        Response::ok(self)
    }
}

impl IntoResponse for &str {
    fn into_response(self) -> Response {
        Response::ok(self)
    }
}

/// Nothing: 200 with no body. Crystal's `Nil` return.
impl IntoResponse for () {
    fn into_response(self) -> Response {
        Response::empty(200)
    }
}

/// A status and nothing else -- `Ok(404)`.
impl IntoResponse for u16 {
    fn into_response(self) -> Response {
        Response::empty(self)
    }
}

/// A body and a status.
impl<T: IntoResponse> IntoResponse for (T, u16) {
    fn into_response(self) -> Response {
        Response {
            status: self.1,
            ..self.0.into_response()
        }
    }
}

/// A body and headers.
impl<T: IntoResponse> IntoResponse for (T, Vec<(String, String)>) {
    fn into_response(self) -> Response {
        let mut response = self.0.into_response();
        response.headers.extend(self.1);
        response
    }
}

/// A body, a status and headers.
impl<T: IntoResponse> IntoResponse for (T, u16, Vec<(String, String)>) {
    fn into_response(self) -> Response {
        let mut response = self.0.into_response();
        response.status = self.1;
        response.headers.extend(self.2);
        response
    }
}

/// What a handler may fail with.
///
/// Deliberately **not** an `impl std::error::Error`, which is what lets the
/// blanket `From` below exist without colliding with the reflexive
/// `impl<T> From<T> for T`. `anyhow::Error` is this shape for the same reason.
/// The effect is that `?` works on anything in a handler.
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub message: String,
}

impl Error {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// The default for a failure nobody classified: it is a bug here, not a
    /// mistake by the caller.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(500, message)
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for Error {
    fn from(error: E) -> Self {
        Self::internal(error.to_string())
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        // Everything a client sees for a 5xx is the status. The message is for
        // whoever is reading the logs, and putting it in the body is how an
        // internal path ends up on a stranger's screen.
        let body = if self.status < 500 {
            self.message
        } else {
            String::from("Internal Server Error")
        };
        Response::new(self.status, body)
    }
}

/// What a handler is given. Middleware sees it too, and may change it before
/// the handler runs or after it returns.
pub struct Ctx {
    pub verb: Verb,
    pub path: String,
    params: Vec<(String, String)>,
    /// How many layout layers the client says it already holds.
    ///
    /// Zero unless a request asked for less, so a first load, a crawler
    /// and a reader with JavaScript off all get the whole document.
    partial: u32,
}

impl Ctx {
    pub fn new(verb: Verb, path: &str, params: Params<'_>) -> Self {
        Self {
            verb,
            path: path.to_string(),
            params: params
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            partial: 0,
        }
    }

    /// The same, for a request that said how much of the page it has.
    #[must_use]
    pub fn holding(mut self, layers: u32) -> Self {
        self.partial = layers;
        self
    }

    /// What `html!` asks, to leave out the layouts the client has.
    ///
    /// A method rather than a public field so the number has one
    /// meaning: layers to skip, counted from the root.
    pub fn partial_depth(&self) -> u32 {
        self.partial
    }

    /// A route parameter, or `None`.
    ///
    /// Never `""`. The Crystal prototype returned an empty string for a name
    /// nobody declared, so a typo read as a present-but-blank value and went
    /// unnoticed; `Option` is that bug deleted rather than documented.
    pub fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The same, for a parameter the route declares and so must be there.
    ///
    /// Returns `Result` rather than `Option` so it composes with `?`. A name
    /// the route does not declare is this file disagreeing with its own
    /// filename, which is a 500 and not a 400: nobody sent a bad request.
    pub fn require(&self, name: &str) -> Result<&str, Error> {
        self.param(name)
            .ok_or_else(|| Error::internal(format!("the route declares no `{name}` parameter")))
    }
}

/// A handler, as the table stores it. One per verb a page answers.
///
/// Boxed rather than a bare `fn` pointer, because every handler returns its
/// own type and a `fn` pointer can only hold one of them. [`page`] is what
/// unifies them, and it runs once per route at startup rather than per
/// request.
pub type Handler = Box<dyn Fn(&mut Ctx) -> Result<Response, Error> + Send + Sync>;

/// Puts a handler in the table whatever it returns.
///
/// Generic over the return type, so `-> Result<impl IntoResponse, Error>` on
/// the handler and one concrete `Handler` in the table are the same thing.
pub fn page<H, T>(handler: H) -> Handler
where
    H: Fn(&mut Ctx) -> Result<T, Error> + Send + Sync + 'static,
    T: IntoResponse,
{
    Box::new(move |ctx| handler(ctx).map(IntoResponse::into_response))
}

/// The application's name, as the root layout writes it.
pub const TITLE: &str = "Nested partial loading";

/// Every template in this application.
///
/// The `.rsx` files sit next to the pages they render -- `index.rsx` beside
/// `index.rs` -- and `load_dir!` compiles the tree while this crate is being
/// compiled. Nothing is read from disk when a request arrives, and a template
/// that does not parse is a compile error on its own line.
///
/// A module of their own, because `load_dir!` emits a `pub mod index` for
/// `index.rsx` and `pages!` emits one for `index.rs`: unnested they collide.
/// The path is relative to the package root and not to this file, because a
/// proc macro cannot learn where it was written on stable Rust.
///
/// A file whose name is not an identifier gets the nearest one: `[id].rsx` is
/// `views::users::_id_`.
pub mod views {
    rsx::load_dir!("src/pages");
}

/// Renders a page inside the layouts that cover it.
///
/// `layout.rsx` wraps every page in its directory and in every directory
/// below it, and they stack from the root down. A page never names one,
/// because which layout covers a page is a property of where the page is --
/// the rule Crystal's page_router uses for `layout.cr`.
///
/// The macro reads that off the directory tree at compile time: it knows
/// which file it was written in, walks up to `src/`, and wraps the page in
/// every `layout.rsx` it passes. Add one to a directory and every page under
/// it is wrapped, with nothing to register and no chain to keep in step.
///
/// Re-exported here so a page imports it from the same place as everything
/// else it uses, and so the layout rule lives beside the route table rather
/// than in a crate name.
pub use brocanth_pages::html;

/// The URL prefix a layout covers, for a layout under a parameter.
///
/// `html!` passes a layout the prefix it wraps, which is the marker dp-swap
/// reads to work out how much of a page two URLs share. A directory named
/// after a parameter has no prefix of its own until a request arrives, so
/// the macro hands this function the template and the request fills it:
/// `/{section}` under `/alpha/card-2` is `/alpha`.
///
/// Called only from the expansion, and only for a layout that sits under a
/// parameter. A marker reading `/[section]` would be the same string on
/// every section, and the client would conclude that two pages share a
/// layer they do not.
pub fn prefix(ctx: &Ctx, template: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        // A parameter the router did not match cannot happen: this template
        // is built from the directory the route was read out of, so the
        // name is one the pattern declares.
        out.push_str(ctx.param(&rest[open + 1..open + close]).unwrap_or_default());
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out
}

/// A partial, and the replacements for the chrome it left behind.
///
/// A layout the client keeps is a layout this answer does not contain, which
/// is the point of asking for a partial and the reason its chrome goes
/// stale: the sidebar still marks the section you came from. A layout that
/// has something to correct defines `Oob` beside its `Layout`, and `html!`
/// renders one per skipped layer and passes them here.
///
/// They lead, because the static version of this writes them that way. The
/// client reads the whole body before it swaps anything, so the order is a
/// convention rather than a requirement, and one convention is better than
/// two.
pub fn render_with(
    out_of_band: Vec<String>,
    content: impl rsx::Template,
) -> Result<Response, Error> {
    let mut body = out_of_band.concat();
    body.push_str(&content.render_to_string()?);
    Ok(Response::ok(body))
}

/// A page with no layout at all: a fragment, a partial, an htmx swap.
///
/// What `html!` expands into, once it has wrapped the page, so this is also
/// the escape hatch -- name a layout by hand and pass it here.
///
/// It returns the handler's own `Result` rather than a value, so a template
/// that bails propagates with `?` instead of turning into a blank 200. For
/// another status or a header, compose with the forms above:
/// `Ok((html!(ctx, tpl)?, 404))`, or `Ok(html!(ctx, tpl)?.with_header("x-kind", "page"))`.
///
/// Deliberately a function and not `impl<T: rsx::Template> IntoResponse for T`.
/// `()`, `Option<T>` and `Vec<T>` are templates, so a blanket impl overlaps
/// the ones above and does not compile -- and an `into_response` that cannot
/// fail would have to swallow a render error. A blanket impl could not apply
/// a layout either, since it would not know which page it was rendering.
pub fn render(content: impl rsx::Template) -> Result<Response, Error> {
    // `render_to_string` builds into its own String, so a page that fails
    // half way through its layout leaves nothing behind. There is no partial
    // body to take back, which is the property the rest of this file is
    // built on.
    Ok(Response::ok(content.render_to_string()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_the_shape_of_the_directory() {
        let router = router().expect("the pages produce a table");
        let mut patterns: Vec<&str> = router
            .routes()
            .iter()
            .map(|(template, _)| template.pattern())
            .collect();
        patterns.sort();
        // This example's own routes. The assertion arrived with the file
        // the scaffold wrote and still named that project's pages, so it
        // asserted `/about` and `/users/{id}` against a directory holding
        // neither, and had been failing since the example was added.
        assert_eq!(
            patterns,
            ["/", "/{section}", "/{section}/{card}", "/{section}/{card}/{tab}"]
        );
    }

    #[test]
    fn a_path_that_exists_with_a_verb_that_does_not_is_a_405() {
        let router = router().expect("the pages produce a table");
        let allow = router.allow_for("/").expect("/ is a route");
        assert_eq!(allow.to_string(), "GET, HEAD");
    }
}
