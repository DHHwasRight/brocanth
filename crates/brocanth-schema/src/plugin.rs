//! The brocanth plugin, and the extraction point.
//!
//! # What the type system is buying here
//!
//! A handler that takes a [`Valid<T>`] has proof the body was checked, because
//! [`Valid`] has a private field and exactly one constructor:
//! [`Validation::check`], a method on the context item this plugin provides.
//! Reaching that item needs `cx.extract()`, and *that* compiles only when the
//! application registered the plugin — the core keeps the installed provider
//! set in the application's type.
//!
//! So forgetting to wire validation is not a bug that reaches production. It
//! is a build that does not finish, and the error names the missing provider.
//! This is §6's "authorization as a typed witness", applied to request bodies.
//!
//! ```
//! use brocanth::plugin_api::Plugin;
//! use brocanth::plugin_app::AppBuilder;
//! use brocanth_schema::plugin::{Validation, ValidationConfig, ValidationPlugin};
//! use brocanth_schema::{Issues, Path, SchemaNode, Validate, Valid};
//!
//! #[derive(Debug)]
//! struct SignUp { email: String }
//!
//! impl Validate for SignUp {
//!     fn schema() -> SchemaNode { SchemaNode::Boolean }
//!     fn validate_at(&self, at: &mut Path, issues: &mut Issues) {
//!         use brocanth_schema::{string, Schema};
//!         at.with_field("email", |at| string().email().check(&self.email, at, issues));
//!     }
//! }
//!
//! // A handler asks for proof, not for a body.
//! fn create(body: Valid<SignUp>) -> String {
//!     body.into_inner().email
//! }
//!
//! let app = AppBuilder::new()
//!     .plugin(ValidationPlugin::new(ValidationConfig::default()))
//!     .build()
//!     .unwrap();
//!
//! let cx = app.context();
//! let validation: &Validation = cx.extract();
//!
//! let good = validation.check(SignUp { email: "a@b.co".into() }).unwrap();
//! assert_eq!(create(good), "a@b.co");
//!
//! let issues = validation.check(SignUp { email: "nope".into() }).unwrap_err();
//! assert_eq!(issues.messages_at("email"), vec!["must be an email address"]);
//! ```
//!
//! Take the `.plugin(..)` line out and the `cx.extract()` line stops
//! compiling. There is nothing to review, because there is nothing to run.

use std::ops::Deref;

use brocanth::plugin_api::{MiddlewareCatalog, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};

use crate::issue::Issues;
use crate::validate::Validate;

/// How a rejected body should be rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReportStyle {
    /// A JSON body, for an API client. See [`Issues::to_json`].
    #[default]
    Json,
    /// Messages grouped by field, for re-rendering a form. See
    /// [`Issues::form_report`].
    Form,
}

/// What [`ValidationPlugin`] is built from.
///
/// A plain value with no parsing in it, as the core requires: the application
/// writes it in `main`, or `brocanth-config` reads it out of a TOML section.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ValidationConfig {
    /// The default rendering for a rejected body.
    pub report: ReportStyle,
}

/// The per-request context item, and the only thing that can mint a
/// [`Valid<T>`].
#[derive(Clone, Copy, Debug)]
pub struct Validation {
    report: ReportStyle,
}

impl ContextItem for Validation {}

impl Validation {
    /// Check `value`, and on success hand back the proof.
    ///
    /// # Errors
    ///
    /// Every issue found, never just the first.
    pub fn check<T: Validate>(&self, value: T) -> Result<Valid<T>, Issues> {
        value.validate().map(|()| Valid(value))
    }

    /// The configured rendering.
    #[must_use]
    pub fn report_style(&self) -> ReportStyle {
        self.report
    }

    /// Render a report the configured way. The JSON form is a body; the form
    /// style is one line per field, which a template usually replaces.
    #[must_use]
    pub fn render(&self, issues: &Issues) -> String {
        match self.report {
            ReportStyle::Json => issues.to_json(),
            ReportStyle::Form => issues
                .form_report()
                .into_iter()
                .map(|(path, messages)| format!("{path}: {}", messages.join(", ")))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

/// Proof that a value passed its schema.
///
/// The field is private and [`Validation::check`] is the only constructor, so
/// a `Valid<T>` in a handler's signature cannot be forged — not by a cast, not
/// by a struct literal, not by `Default`.
///
/// ```compile_fail
/// use brocanth_schema::Valid;
///
/// struct SignUp { email: String }
///
/// // There is no public constructor, so there is no way in.
/// let forged = Valid(SignUp { email: "nope".into() });
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Valid<T>(T);

impl<T> Valid<T> {
    /// The checked value.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// The checked value, borrowed.
    pub fn get(&self) -> &T {
        &self.0
    }
}

impl<T> Deref for Valid<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

/// The plugin an application names in `main`.
///
/// It provides [`Validation`] and offers one middleware. It does not mount it:
/// mounting takes a pattern and the pattern is the application's decision.
#[derive(Clone, Copy, Debug)]
pub struct ValidationPlugin {
    settings: Validation,
}

impl Plugin for ValidationPlugin {
    type Config = ValidationConfig;
    type Provides = ProvidesContext<Validation>;

    const NAME: &'static str = "validation";

    fn new(config: Self::Config) -> Self {
        Self {
            settings: Validation {
                report: config.report,
            },
        }
    }

    fn configure_router(&mut self, catalog: &mut MiddlewareCatalog<'_>) -> Result<(), PluginError> {
        catalog.offer(
            "validation.report",
            "Turns an unhandled validation report into a 422 rendered the configured way.",
        );
        Ok(())
    }

    fn provide(&self, cx: &mut ContextBuilder) {
        cx.insert(self.settings);
    }
}

#[cfg(test)]
mod tests {
    use brocanth::plugin_app::{App, AppBuilder};
    use brocanth::plugin_context::{Base, With};

    use crate::{string, Issues, Path, Schema, SchemaNode};

    use super::*;

    #[derive(Debug)]
    struct SignUp {
        email: String,
    }

    impl Validate for SignUp {
        fn schema() -> SchemaNode {
            SchemaNode::Boolean
        }

        fn validate_at(&self, at: &mut Path, issues: &mut Issues) {
            at.with_field("email", |at| {
                string().email().check(&self.email, at, issues);
            });
        }
    }

    type Providers = With<Validation, Base>;

    fn app() -> App<Providers> {
        AppBuilder::new()
            .plugin(ValidationPlugin::new(ValidationConfig::default()))
            .build()
            .unwrap()
    }

    #[test]
    fn the_context_item_is_reachable_once_the_plugin_is_registered() {
        let app = app();
        let cx = app.context();
        let validation: &Validation = cx.extract();
        assert_eq!(validation.report_style(), ReportStyle::Json);
    }

    #[test]
    fn a_rejected_body_never_becomes_a_valid() {
        let app = app();
        let cx = app.context();
        let validation: &Validation = cx.extract();
        let issues = validation
            .check(SignUp {
                email: "nope".into(),
            })
            .unwrap_err();
        assert!(validation.render(&issues).contains("\"path\":\"email\""));
    }

    #[test]
    fn the_middleware_is_offered_and_not_mounted() {
        let app = app();
        let plan = app.router_plan();
        assert!(plan.offer_named("validation.report").is_some());
        assert!(plan.mounts().is_empty());
    }
}
