//! `routes!`: the whole route table, written in one file.
//!
//! brocanth's first answer to "where is a route" is a file under `src/pages`.
//! That answer is right for a page tree and wrong for two kinds of project:
//! an API, which has no pages, and a team that wants every route on one
//! screen. This macro is the second answer. Both fill the same
//! [`RouterBuilder`](brocanth::routing::RouterBuilder), so a path declared
//! twice is one error at startup rather than a race between two tables.
//!
//! ```ignore
//! // src/routes.rs
//! use crate::pages::{page, Handler};
//!
//! brocanth_routes::routes! {
//!     root                 => pages::index::handle;
//!     get  "/about"        => pages::about::handle;
//!     post "/signup"       => pages::signup::handle_post;
//!
//!     resources "/posts"   => crate::controllers::posts;
//!     resources "/tags"    => crate::controllers::tags, only: [index, show];
//!
//!     scope "/admin" {
//!         get "/" => crate::admin::dashboard;
//!     }
//! }
//! ```
//!
//! # What it emits
//!
//! One function, exactly the one every route source in a project offers:
//!
//! ```ignore
//! pub fn mount(
//!     routes: &mut brocanth::routing::RouterBuilder<Handler>,
//!     prefix: &str,
//! ) -> Result<(), brocanth::routing::BuildError>;
//! ```
//!
//! and a `routes` module beside it holding one `path()` per route, taking
//! exactly that route's parameters. `Handler` and `page` are the project's
//! own, from `src/pages/mod.rs`, so the file that calls this macro imports
//! them first. The macro names them and nothing else, which is what lets the
//! same table serve a project whose `Ctx` and `Error` are its own.
//!
//! # The rules
//!
//! | Line | What it adds |
//! | ---- | ------------ |
//! | `root => h;` | `GET /` |
//! | `get "/about" => h;` | `GET /about`. Also `head`, `post`, `put`, `patch`, `delete`, `options` |
//! | `resources "/posts" => ctrl;` | the seven below |
//! | `resource "/profile" => ctrl;` | the same six, without `index` and without `{id}` |
//! | `scope "/admin" { .. }` | a URL prefix, and the helper modules to match |
//! | `namespace admin { .. }` | the same, written as one identifier |
//! | `member { .. }` / `collection { .. }` | inside a resource: under `/posts/{id}` or under `/posts` |
//!
//! `resources "/posts" => ctrl` is these seven, in this order:
//!
//! | Action | Verb | Path |
//! | ------ | ---- | ---- |
//! | `ctrl::index` | GET | `/posts` |
//! | `ctrl::new` | GET | `/posts/new` |
//! | `ctrl::create` | POST | `/posts` |
//! | `ctrl::show` | GET | `/posts/{id}` |
//! | `ctrl::edit` | GET | `/posts/{id}/edit` |
//! | `ctrl::update` | PATCH and PUT | `/posts/{id}` |
//! | `ctrl::destroy` | DELETE | `/posts/{id}` |
//!
//! Four paths, eight registrations: `index` and `create` share `/posts`, and
//! `show`, `update` and `destroy` share `/posts/{id}`, which is what makes
//! 405 work there.
//!
//! # Why a controller is a module and not seven arms
//!
//! Rails infers `PostsController#index` from the name. This macro infers
//! `ctrl::index` from the module, which looks like the same bargain and is
//! not: the inferred name is emitted as a path, so a controller missing
//! `edit` is `error[E0425]: cannot find function `edit``, pointing at the
//! `resources` line. Seven explicit arms would state what the compiler can
//! already check, on every resource in the file. `only:` and `except:` are
//! how a controller says it has four actions rather than seven -- a rule the
//! table enforces, not a comment.
//!
//! # Options
//!
//! | Option | On | Meaning |
//! | ------ | -- | ------- |
//! | `only: [index, show]` | resource | keep these actions |
//! | `except: [destroy]` | resource | drop these actions |
//! | `param: slug` | resource | the member parameter, `id` by default |
//! | `shallow: true` | resource | nested member routes hang off the root, not off the parent |
//! | `as: name` | any route or group | the helper module to use, instead of the path |
//!
//! # What a group nests, and what it leaves alone
//!
//! A group nests two things: the URL, and the helper modules under it. They
//! move together, so `routes::admin::users::path()` builds `/admin/users` and
//! a reader can predict either from the other. That is the promise the file
//! side already keeps, where `src/pages/admin/users.rs` is `/admin/users`.
//!
//! A group nests neither the handler path nor anything else. A handler is an
//! item the author wrote; splicing `admin::` in front of it would build a
//! path that appears in no file and that no editor can follow to its
//! definition. The URL is this macro's business, the module path is the
//! compiler's.
//!
//! That leaves `namespace admin` as `scope "/admin"` written as one word, and
//! the macro treats them as the same rule. Each is better at one thing: a
//! scope takes a path, so it nests several segments at once and may carry a
//! parameter; a namespace takes an identifier, so its prefix and its helper
//! module cannot drift apart.
//!
//! # What it will not do
//!
//! Constraints (a regex on a segment, a subdomain) are not here. brocanth's
//! [`RouteTemplate`](brocanth::routing::RouteTemplate) accepts `{name}` and
//! `{*name}` and nothing else, so a constraint would have to be a change in
//! the routing core rather than in this macro.

mod expand;
mod parse;

/// The whole route table, in one block.
///
/// See the crate documentation for the grammar, the seven routes a
/// `resources` line produces, and what `scope` and `namespace` each nest.
///
/// Emits `pub fn mount(..)` and a `pub mod routes` of path helpers. `Handler`
/// and `page` must be in scope where the macro is called.
#[proc_macro]
pub fn routes(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    expand::expand(input.into()).into()
}
