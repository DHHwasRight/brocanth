//! `openapi` -- what `brocanth create` writes when somebody asks for a
//! document.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that [`SpecBuilder`](crate::SpecBuilder)
//! takes a `Router` and gives a `Document`, that
//! [`OpenApiConfig`](crate::OpenApiConfig) holds a whole `Document` and
//! therefore cannot be read out of a TOML file, that
//! [`OpenApiPlugin::configure`] is where an invalid document becomes a build
//! failure. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! # Why the generated plugin is not `OpenApiPlugin`
//!
//! The same split as the cache and the queue, for the same reason.
//! [`OpenApiPlugin`](crate::OpenApiPlugin) is handed a document that is
//! already built, because `Plugin::new` cannot fail and deriving a document
//! can. A new project has a title and a version in a config file, so the
//! generated plugin takes those two strings, derives the document from the
//! project's own route table at `configure`, and holds an `OpenApiPlugin`
//! built from the result. Swapping to `OpenApiPlugin` directly is a change in
//! one generated file.
//!
//! # Why the endpoint is a page and not a mount
//!
//! [`OpenApiPlugin::mount`](crate::OpenApiPlugin::mount) takes the
//! application's own `RouterBuilder`, and a generated application does not
//! have one: its table is built by `brocanth_pages::pages!` from the shape of
//! `src/pages`, and `src/app.rs` dispatches that table and nothing else. A
//! route in such a project is a file. So the endpoint is a file --
//! `src/pages/openapi.json.rs` -- and it reads a body this module's plugin
//! rendered once at startup.
//!
//! That is not a workaround for a missing seam; it is the seam. The path is
//! still the application's decision, it is still one thing a reader can
//! delete, and the document that describes the table is served by a row of
//! the table it describes -- which is why the generated document hides it.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.
//!
//! [`OpenApiPlugin::configure`]: crate::OpenApiPlugin

use brocanth_scaffold::snippet::{plugin, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// What the plugin does with `pages::described()`.
///
/// Written out here rather than inline in the template so it reads as
/// Rust: the format string around it escapes every brace.
const BODIES: &str = r#"
        // What each page takes and gives. `pages!` read it off the handler
        // signatures, so the document says what the code does rather than
        // what somebody remembered to write down.
        for route in crate::pages::described() {
            if let Some(node) = &route.takes {
                spec = spec.describing_node(node);
            }
            if let Some(node) = &route.gives {
                spec = spec.describing_node(node);
            }
            let takes = route.takes.as_ref().and_then(|node| node.name());
            let gives = route.gives.as_ref().and_then(|node| node.name());
            spec = spec.describe(route.path, route.verb, move |mut op| {
                if let Some(name) = takes {
                    op = op.json_body(Schema::reference(name));
                }
                if let Some(name) = gives {
                    // A description is required and may not be empty, which
                    // the document's own validator says before anything is
                    // served. One sentence, the same for every operation,
                    // and `.describe(..)` is where a better one goes.
                    op = op.json_response(200, "The answer", Schema::reference(name));
                }
                op
            });
        }
"#;

/// The document, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct OpenApi;

impl Module for OpenApi {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "openapi",
            "an OpenAPI 3.1 document derived from src/pages, served at /openapi.json",
        )
        .asked_in("doors", "An OpenAPI document, derived from your routes")
        .krate(
            Dep::brocanth("brocanth-openapi")
                // The bodies come out of `zod` nodes, which is the one
                // way this document learns what a page takes and gives.
                // Declared here and not by whether `input` was chosen,
                // because a manifest is read before the picks are known.
                .feature("zod"),
        )
        // Only for the order of the `[openapi]` section and the builder line:
        // the title and the version come from the file the config plugin
        // loaded, so this belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-openapi",
            "no document; a client learns this API by reading the handlers",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/openapi/mod.rs", plugin_rs(project));
        // The endpoint. A route in this framework is a file under
        // `src/pages`, and the last dot is the extension, so this file is
        // `/openapi.json` and renaming it moves the document.
        s.file("src/pages/openapi.json.rs", PAGE_RS);
        // And the same document as YAML. Two files because two routes,
        // and because a `yaml()` nothing called was a function the
        // compiler warned about on a generated project's first build.
        s.file("src/pages/openapi.yaml.rs", YAML_PAGE_RS);
        // Two pages, so two routes, and the table's own test lists every
        // route a project has. Until this slot existed the test was a
        // literal three entries and `--with openapi` shipped a project
        // whose `cargo test` failed on the day it was generated.
        s.at(Slot::PagesRoutes).line("/openapi.json");
        s.at(Slot::PagesRoutes).line("/openapi.yaml");
        s.at(Slot::PluginsMod).line("pub mod openapi;");
        plugin(s, project, "openapi", "OpenApi");
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait method,
            // so the trait has to be in scope. Every plugin module needs the
            // same line, and `use` twice does not compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(section(project));
        }
    }
}

/// The `[openapi]` section of `config/development.toml`.
fn section(project: &Project) -> String {
    format!(
        "\n[openapi]\n\
         # The name of the API, which is not the name of this binary: the\n\
         # document is read by somebody who never sees the source.\n\
         title = {title:?}\n\
         # The version of the contract, not of the crate. It moves when an\n\
         # operation changes and stays still when the code does.\n\
         version = \"0.1.0\"\n",
        title = project.title
    )
}

/// `src/pages/openapi.json.rs`: the one route that serves the document.
const YAML_PAGE_RS: &str = r#"//! `/openapi.yaml` -- the same document, in the other spelling.
//!
//! Two files and not one handler branching on `Accept`. A route here is a
//! file, so two routes are two files, and a reader who wants only one of
//! them deletes the other.
//!
//! YAML because a person reads this one. The JSON is what a generator
//! consumes; this is what somebody opens to find out whether the endpoint
//! they were promised exists.

use crate::pages::{Ctx, Error, IntoResponse};

/// GET `/openapi.yaml`.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let body = crate::plugins::openapi::yaml().ok_or_else(|| {
        Error::internal("the openapi plugin is not registered, so there is no document to serve")
    })?;
    Ok((
        body.to_string(),
        200,
        vec![
            ("content-type".to_string(), "application/yaml".to_string()),
            ("cache-control".to_string(), "no-cache".to_string()),
        ],
    ))
}
"#;

const PAGE_RS: &str = r#"//! `/openapi.json` -- because the file is called `openapi.json.rs`.
//!
//! The route is the file name, as it is for every page: the last dot is the
//! extension and what is left is the path, so this file answers
//! `/openapi.json` and renaming it moves the document.
//!
//! The body was rendered once, at startup, by `src/plugins/openapi/mod.rs`.
//! This page hands it out and does nothing else.

use crate::pages::{Ctx, Error, IntoResponse};

/// GET `/openapi.json`.
///
/// `no-cache` rather than a long max-age: the document changes when this
/// application is redeployed, and a client holding a stale one is the exact
/// failure a derived document exists to prevent.
pub fn handle(_ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let body = crate::plugins::openapi::json().ok_or_else(|| {
        Error::internal("the openapi plugin is not registered, so there is no document to serve")
    })?;
    Ok((
        body.to_string(),
        200,
        vec![
            ("content-type".to_string(), "application/json".to_string()),
            ("cache-control".to_string(), "no-cache".to_string()),
        ],
    ))
}
"#;

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "openapi");

    // With `input`, a page's signature says what it takes and gives, and
    // `pages::described()` is that table. Without it there is no table, so
    // the document is paths and verbs and the honest sentence saying so.
    // `Schema` is named only inside `BODIES`, and `spec` is only rebound
    // there. Without `input` both were written anyway, so every
    // `--with openapi` project warned twice on its first build.
    let (bodies, bodies_code, schema_use, spec_binding) = if project.has("input") {
        (
            " What a page takes and gives comes\n//! from `pages::described()`, which `pages!` wrote off the handler\n//! signatures: a second parameter is the request body, a `Json<T>`\n//! return is the answer. Neither is written here, so neither can be\n//! out of date.",
            BODIES,
            ", Schema",
            "let mut spec",
        )
    } else {
        (
            " What a page takes and gives is not\n//! derivable from a `Fn(&mut Ctx)`, so a body is added by hand.\n//! `brocanth create --with input` is what makes the signature say\n//! it instead.",
            "",
            "",
            "let spec",
        )
    };

    format!(
        r##"//! The OpenAPI document, as this application derives it.
//!
//! The route table already knows every path, every verb and every path
//! parameter, and it knows them while this crate compiles, because the
//! filesystem is the route table. So the document is derived from
//! `crate::pages::router()` rather than written, and the two cannot drift:
//! add a file under `src/pages` and the document grows an operation.
//!
//! `brocanth-openapi` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/app.rs`, this file
//! and `src/pages/openapi.json.rs`, and the application describes nothing and
//! links nothing to describe it with.
//!
//! # What is derived, and what is not
//!
//! Every route, with its path parameters, an `operationId` and a 200.{bodies}
//!
//! Anything else -- a summary, a second status, an example -- is added by
//! hand with `SpecBuilder::describe`, beside the lines below:
//!
//! ```ignore
//! .describe("/users/{{id}}", Verb::Get, |op| {{
//!     op.summary("One user")
//! }})
//! ```
//!
//! `/openapi.json` is hidden from the document it serves. The endpoint is a
//! row of the table it describes, and an endpoint that describes itself is
//! noise in every client somebody generates from this.
//!
//! # Why this is not `OpenApiPlugin`
//!
//! The crate also ships `OpenApiPlugin`, and this is not that. That one is
//! handed a document that is already built, which is right for an application
//! that writes its document in `main`. This one takes a title and a version,
//! because a new project has a title and a version. It still holds an
//! `OpenApiPlugin` and calls its hooks, so the two things only that type can
//! do still happen: an invalid document stops the build, and the document
//! reaches `Services` for anything else that wants to read it.

use std::sync::OnceLock;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::NoContext;
use brocanth::routing::Verb;
use brocanth_openapi::{{Format, OpenApiConfig, OpenApiPlugin{schema_use}, SpecBuilder}};

/// Where the document is served.
///
/// A constant and not a setting. The path is the name of the file in
/// `src/pages` that answers it, so moving the document is renaming that file
/// and this line, and a setting that disagreed with the file name would serve
/// a 404 and hide it in the document.
pub const ROUTE: &str = "/openapi.json";

/// The rendered document, both ways round.
///
/// Rendered once at `configure`, so a request costs a borrow rather than a
/// walk of the whole structure. A `OnceLock` because the page that serves it
/// is a plain function with no handle on this plugin.
static RENDERED: OnceLock<(String, String)> = OnceLock::new();

/// The document as JSON, or `None` when this plugin was never registered.
pub fn json() -> Option<&'static str> {{
    RENDERED.get().map(|(json, _)| json.as_str())
}}

/// The document as YAML, or `None` when this plugin was never registered.
///
/// Served by `src/pages/openapi.yaml.rs`, which is the file beside the
/// JSON one and reads this instead.
pub fn yaml() -> Option<&'static str> {{
    RENDERED.get().map(|(_, yaml)| yaml.as_str())
}}

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// The name of the API, which is not the name of this binary.
    pub title: String,
    /// The version of the contract, not of the crate.
    pub version: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            title: {title:?}.to_string(),
            version: "0.1.0".to_string(),
        }}
    }}
}}

pub struct OpenApi {{
    settings: Settings,
    /// Built at `configure` and not in `new`, because deriving a document can
    /// fail and `Plugin::new` cannot.
    served: Option<OpenApiPlugin>,
}}

impl Plugin for OpenApi {{
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "openapi";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            served: None,
        }}
    }}

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The same function `src/app.rs` builds the live table with, so the
        // document describes what this binary actually dispatches.
        let router = crate::pages::router().map_err(|e| {{
            PluginError::new(format!("[openapi] the route table does not build: {{e}}"))
        }})?;
        {spec_binding} = SpecBuilder::new(self.settings.title.clone(), self.settings.version.clone())
            .routes_from_router(&router)
            .hide(ROUTE, Verb::Get);
{bodies_code}
        let document = spec
            .build()
            .map_err(|e| PluginError::new(format!("[openapi] deriving the document: {{e}}")))?;

        // The crate's own plugin, held rather than registered. Its
        // `configure` is what makes an invalid document a build failure
        // instead of a 500 found during an incident, and it renders both
        // bodies once.
        let mut served = OpenApiPlugin::new(OpenApiConfig::new(document).json_route(ROUTE));
        served.configure(lc)?;
        let _ = RENDERED.set((
            served.body(Format::Json).to_string(),
            served.body(Format::Yaml).to_string(),
        ));
        self.served = Some(served);
        Ok(())
    }}

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // Publishes the document through `Services`, so a command or another
        // plugin can read it with `brocanth_openapi::document_of`.
        match &mut self.served {{
            Some(served) => served.register(lc),
            None => Ok(()),
        }}
    }}
}}
"##,
        title = project.title
    )
}
