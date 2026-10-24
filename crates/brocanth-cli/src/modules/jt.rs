//! `jt`, the second renderer.
//!
//! This module exists to be small. `rsx` was a special case written into
//! the generator by hand, and the argument that it should not be could only
//! be settled by a second engine attaching through the seam instead. What
//! follows is the whole attachment: a manifest, a dependency, one block
//! into [`Slot::PagesRenderers`] and two files.
//!
//! Nothing in `brocanth-pages`, `create.rs` or the route table knows this
//! module exists.
//!
//! # Why a JSON template at all
//!
//! A handler that answers JSON has two ways to do it already. Build a
//! struct and derive something, which costs a type per response shape and
//! a serializer at run time. Or write the string, which nothing checks.
//!
//! `jt` is the third: the template is the shape, it is compiled into
//! straight-line writes while the crate builds, and a template that does
//! not parse is a compile error on its own line. It is the same bargain
//! `rsx` makes for HTML, which is why it attaches at the same place.
//!
//! # The one thing it does not do yet
//!
//! A `.jt` template is an output schema -- `jt-core` holds the keys, the
//! nesting and which fields are optional -- and `brocanth-openapi` wants
//! exactly that. Today a page says what it gives through
//! `Result<Json<T>, Error>` and a `zod::Node`. A `.jt` page says nothing,
//! so it appears in the document with no response schema. Closing that is
//! worth its own change; see `docs/modules.md` §5.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// `jt` lives in its own repository, like `rsx`: brocanth is the thing
/// being developed here and wants a path, a template engine is an ordinary
/// dependency and is fetched.
pub const REPOSITORY: &str = "https://github.com/DHHwasRight/jt.git";

/// JSON from `.jt` templates, compiled into the binary.
pub struct Jt;

impl Module for Jt {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "jt",
            "`.jt` templates compiled into the binary; a JSON answer without a struct per shape",
        )
        .asked_in("pages", "JSON from templates, compiled into the binary")
        .krate(Dep::git("jt", REPOSITORY, "main").note("JSON templates, compiled"))
        .absent(
            "jt",
            "no JSON templates; a page that answers JSON builds the String \
             it returns, or derives a struct for the shape",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        // The renderer's half of `src/pages/mod.rs`. `rsx` writes its own
        // beside this one and the two do not meet: each `load_dir!` reads
        // the same tree and compiles only its own extension.
        s.at(Slot::PagesRenderers).block(PAGES_JT_RS);

        // A page and its template, beside each other, the same way an
        // `.rsx` file sits beside the `.rs` file that renders it.
        s.file("src/pages/status.rs", STATUS_RS);
        s.file("src/pages/status.jt", STATUS_JT);
        // A route is a file, so a module that writes a page adds a route,
        // and the table's own test has to know.
        s.at(Slot::PagesRoutes).line("/status");
    }
}

/// What `jt` adds to `src/pages/mod.rs`.
///
/// Two items: the compiled tree, and the one function that turns a template
/// into a response. `rsx`'s block is longer because HTML has layouts and
/// JSON does not -- there is no document to wrap a document in.
const PAGES_JT_RS: &str = r##"/// Every JSON template in this application.
///
/// The `.jt` files sit next to the pages they answer -- `status.jt` beside
/// `status.rs` -- and `load_dir!` compiles the tree while this crate is
/// being compiled. Nothing is parsed when a request arrives, and a template
/// that does not parse is a compile error on its own line.
///
/// A module of their own, for the same reason the templates have one:
/// `load_dir!` emits a `pub mod status` for `status.jt` and `pages!` emits
/// one for `status.rs`, so unnested they collide. The path is relative to
/// the package root and not to this file, because a proc macro cannot learn
/// where it was written on stable Rust.
///
/// A file whose name is not an identifier gets the nearest one, by the same
/// rule the templates and the route table use: `[id].jt` is
/// `payload::users::_id_`.
pub mod payload {
    jt::load_dir!("src/pages");
}

/// Renders a JSON template as the answer.
///
/// The counterpart of `render` above, and deliberately a second function
/// rather than a second `impl IntoResponse`: a template is an ordinary
/// value, so a blanket impl would claim types that are not answers.
///
/// It returns the handler's own `Result` so a page reads the same whichever
/// engine it uses. For another status or a header, compose:
/// `Ok(render_json(tpl)?.with_header("cache-control", "no-store"))`.
///
/// To override the type this sets, reach for `set_header` and not
/// `with_header`: the second appends, so two `content-type` values would
/// reach the wire and the client would pick between them.
pub fn render_json(content: impl jt::ToJson) -> Result<Response, Error> {
    // Into its own String first, so a page that fails half way through
    // leaves nothing behind. Same property the HTML path has.
    Ok(Response::ok(jt::to_string(&content)).with_header("content-type", "application/json"))
}
"##;

/// `src/pages/status.rs`: the handler, which is three lines because the
/// shape is in the template beside it.
const STATUS_RS: &str = r##"//! `/status` -- because the file is called `status.rs`.
//!
//! The answer is JSON, and its shape is in `status.jt` beside this file.
//! Nothing here builds a struct for the response: the template is the
//! shape, and it is compiled into the binary rather than parsed per
//! request.

use crate::pages::{payload, render_json, Ctx, Error, IntoResponse};

/// GET. The verb comes off the function name, the same as every other page.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    render_json(payload::status::Status(
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        &["pages", "router"],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_is_the_shape_of_the_answer() {
        // Rendered without a request, because a template takes values and
        // not a context. Edit `status.jt` and this is what changes.
        let body = jt::to_string(&payload::status::Status("api", "0.1.0", &["pages"]));
        assert_eq!(
            body,
            r#"{"name":"api","version":"0.1.0","ok":true,"parts":["pages"]}"#
        );
    }

    #[test]
    fn the_answer_says_it_is_json() {
        let answered = render_json(payload::status::Status("api", "0.1.0", &[])).expect("renders");
        assert!(answered
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/json"));
    }
}
"##;

/// `src/pages/status.jt`: the shape of the answer.
const STATUS_JT: &str = r#"// The body of `/status`. A template is a function whose body is JSON with
// holes, and the macro compiles it into straight-line writes: every static
// run below is one memcpy, and the commas are resolved while this file is
// compiled rather than counted when a request arrives.
//
// Take parameters by reference. A `&str` is borrowed into the template and
// nothing is copied to render it.

pub fn Status(name: &str, version: &str, parts: &[&str]) -> Json {
    "name": name,
    "version": version,
    "ok": true,
    "parts": [ for p in parts { p } ],
}
"#;
