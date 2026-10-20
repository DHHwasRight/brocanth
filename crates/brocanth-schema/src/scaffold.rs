//! `validation` -- what `brocanth create` writes when somebody asks for
//! request validation.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that the context item is
//! [`Validation`](crate::Validation) and the only thing that can mint a
//! [`Valid<T>`](crate::Valid), that
//! [`ReportStyle`](crate::ReportStyle) has exactly two spellings, that
//! [`ValidationConfig`](crate::ValidationConfig) is a plain value with no
//! parser in it and therefore cannot be read out of a TOML file by itself.
//! Those move when this crate moves, and a generator living in another
//! repository would learn about it from a bug report.
//!
//! # Why the generated plugin is not `ValidationPlugin`
//!
//! Because `ValidationConfig` does not deserialise. Its own documentation
//! says "the application writes it in `main`, or `brocanth-config` reads it
//! out of a TOML section", and the second half is not true today: the struct
//! carries no `Deserialize`, and this crate's `serde` feature covers
//! `SchemaNode`, `Issue` and `Issues` and not the config. So the generated
//! plugin owns a `Settings` that does deserialise -- one string -- turns it
//! into a `ReportStyle` at `configure`, where a typo can still be answered,
//! and hands the result to a `ValidationPlugin` it holds. The same split as
//! the cache, for the same reason.
//!
//! # What it deliberately does not mount
//!
//! `ValidationPlugin::configure_router` offers a middleware called
//! `validation.report`, and this module does not pass that offer on. The
//! offer is a name and a sentence; the crate ships no function behind it,
//! because turning an unhandled report into a 422 means knowing what the
//! application's `Response` is. A generated project would therefore mount a
//! name it has no layer for, which stops the application at startup. The
//! generated file says this out loud, next to the two lines that would
//! change it.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The validation vocabulary, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Validation;

impl Module for Validation {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "validation",
            "a zod-shaped vocabulary; a handler takes Valid<T> or does not compile",
        )
        .krate(Dep::brocanth("brocanth-schema"))
        // Only for the order of the `[validation]` section and the builder
        // line: the report style comes from the file the config plugin
        // loaded, so this belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-schema",
            "no schema vocabulary; a body is checked by hand, or it is not checked",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/validation/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod validation;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "validation", "Validation"));
        // The context item, read once at startup. It is a demonstration and
        // it is load-bearing: the installed provider set lives in the
        // application's type, so this line stops compiling the moment the
        // registration above is deleted.
        s.at(Slot::AppContext).block(
            "    let checks: &brocanth_schema::Validation = cx.extract();\n\
            \x20   eprintln!(\"validation: reports as {:?}\", checks.report_style());"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait method,
            // so the trait has to be in scope. Every plugin module needs the
            // same line, and `use` twice does not compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(SECTION.to_string());
        }
    }
}

/// The `[validation]` section of `config/development.toml`.
const SECTION: &str = "\n[validation]\n\
                       # json for an API client, form for re-rendering a form with the\n\
                       # messages grouped by field. A third spelling stops the\n\
                       # application at startup rather than reporting the wrong way.\n\
                       report = \"json\"\n";

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "validation");
    format!(
        r##"//! Request validation, as this application registers it.
//!
//! The shape of a schema is checked when this crate compiles; the data is
//! checked when a request arrives, because a request body is bytes that did
//! not exist at compile time. What the derive buys is that the check and the
//! description come from one declaration, so a form, an OpenAPI document and
//! the check on the way in cannot drift apart.
//!
//! `brocanth-schema` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/app.rs` and delete
//! this file, and the application validates nothing and links nothing to
//! validate with.
//!
//! # The proof, and why a handler asks for it
//!
//! `Valid<T>` has a private field and exactly one constructor:
//! `Validation::check`, a method on the context item this plugin provides.
//! A handler that takes a `Valid<SignUp>` therefore has proof the body was
//! checked, and a handler in an application that never registered this plugin
//! does not compile. Forgetting to wire validation is a build that does not
//! finish rather than a bug that reaches production.
//!
//! ```ignore
//! let checks: &brocanth_schema::Validation = cx.extract();
//! match checks.check(SignUp {{ username, email }}) {{
//!     Ok(body) => create(body),
//!     Err(issues) => Response {{
//!         status: 422,
//!         body: checks.render(&issues),
//!         ..Default::default()
//!     }},
//! }}
//! ```
//!
//! # Where schemas go
//!
//! `SignUp` is here so that this file compiles into something that runs. Move
//! it, and whatever follows it, into `src/schemas/` once there is more than
//! one. Nothing registers a schema: a type with `#[derive(Validate)]` is
//! reachable from wherever it is written.
//!
//! # The middleware this does not mount
//!
//! `ValidationPlugin` offers a middleware called `validation.report`, and
//! this plugin does not pass the offer on. The offer is a name; the crate
//! ships no function behind it, because turning an unhandled report into a
//! 422 means knowing what this application's `Response` is. Mounting a name
//! with no layer behind it stops the application at startup, so the offer is
//! withheld until there is something to mount. Write the layer in this file,
//! add `"validation.report" => Some(validation::layer),` to
//! `src/plugins/mod.rs`, and forward `configure_router` to `self.inner`.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ProvidesContext}};
use brocanth_schema::{{ReportStyle, Validate, ValidationConfig, ValidationPlugin}};

// ---------------------------------------------------------------------------
// A schema
// ---------------------------------------------------------------------------

/// What somebody may send to sign up.
///
/// A constraint that cannot mean anything for a field's type does not
/// compile, and neither does a rule naming a field this struct does not have:
/// the derive reads the field list from the struct, so there is no string to
/// misspell.
#[derive(Debug, Validate)]
/// The shape a sign-up body has to have.
///
/// Allowed to be unconstructed: a handler is what builds one, and a
/// handler cannot reach the per-request context yet — `Ctx` in
/// `src/pages/mod.rs` has no extractor, so `Valid<SignUp>` has nowhere to
/// be asked for. Delete this attribute with the first route that takes a
/// body.
#[allow(dead_code)]
pub struct SignUp {{
    /// Three to thirty-two characters.
    #[validate(string(min_length = 3, max_length = 32))]
    pub username: String,
    /// Checked against the shape of an address, not against a mail server.
    #[validate(string(email))]
    pub email: String,
}}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `json` or `form`. A string here and an enum below: the config file is
    /// text, and the translation is checked at startup where a typo can still
    /// be answered.
    pub report: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            report: "json".to_string(),
        }}
    }}
}}

pub struct Validation {{
    report: String,
    /// Built at `configure` and not in `new`, because reading the report
    /// style out of a string can fail and `Plugin::new` cannot.
    inner: Option<ValidationPlugin>,
}}

impl Plugin for Validation {{
    type Config = Settings;
    type Provides = ProvidesContext<brocanth_schema::Validation>;
    const NAME: &'static str = "validation";

    fn new(config: Settings) -> Self {{
        Self {{
            report: config.report,
            inner: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let report = match self.report.as_str() {{
            "json" => ReportStyle::Json,
            "form" => ReportStyle::Form,
            other => {{
                return Err(PluginError::new(format!(
                    "[validation] report is {{other:?}}; write json or form"
                )))
            }}
        }};
        // The crate's own plugin, held rather than registered: it is the only
        // thing that can build the context item, because `Validation`'s field
        // is private and this file is not that crate.
        self.inner = Some(ValidationPlugin::new(ValidationConfig {{ report }}));
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the inner plugin is here by the time any request builds a context.
        if let Some(inner) = &self.inner {{
            inner.provide(cx);
        }}
    }}
}}
"##
    )
}
