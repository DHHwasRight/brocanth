//! Serving the document, as a plugin that must be named in `main`.
//!
//! # What the core owes a plugin, and what it does not
//!
//! §8 of the plan: the dependency arrow points from the plugin to the core,
//! always. This crate depends on `brocanth`; `brocanth` has never heard of
//! it. An application that does not write `.plugin(OpenApiPlugin::new(..))`
//! links no part of this file, and its binary contains no document.
//!
//! The core publishes five lifecycle hooks and this plugin uses two of them.
//! [`Plugin::configure`] validates the document and renders both bodies once,
//! so a request costs a clone of a `String` rather than a walk of the whole
//! structure, and so an invalid document stops the build rather than
//! appearing as a 500 the first time somebody fetches it.
//! [`Plugin::register`] publishes the document through
//! [`Services`](brocanth::plugin_api::Services), so a plugin that wants to
//! read it — a CLI command, a documentation page — can.
//!
//! # Mounting
//!
//! The core's router is generic over the handler type, and there is no HTTP
//! server layer yet, so this plugin cannot register its own routes and would
//! not if it could: *"I do not want plugins to register middleware
//! themselves."* The same reasoning applies to routes. So the plugin offers
//! [`OpenApiPlugin::mount`], which the application calls with its own handler
//! type, at paths the application chose.
//!
//! ```
//! use brocanth::routing::{Router, Verb};
//! use brocanth_openapi::{Document, Format, OpenApiConfig, OpenApiPlugin};
//! use brocanth::plugin_api::Plugin;
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let plugin = OpenApiPlugin::new(OpenApiConfig::new(Document::new("Blog", "1.0.0")));
//!
//! // The application's own handler type, whatever it is.
//! let mut builder: brocanth::routing::RouterBuilder<Format> = Router::builder();
//! plugin.mount(&mut builder, |format| format)?;
//! let router = builder.build()?;
//!
//! assert!(router.at("/openapi.json").is_some());
//! # Ok(())
//! # }
//! ```
//!
//! The document itself should not describe those two routes — they are not
//! part of the API. Hide them when the document is built:
//! `.hide("/openapi.json", Verb::Get)`.

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError, Services};
use brocanth::plugin_context::NoContext;
use brocanth::routing::{BuildError, RouterBuilder, Verb};

use crate::document::Document;
use crate::drift::{render, Format};

/// The document, shared with anything that asks the application for it.
pub type SharedDocument = Arc<Document>;

/// The document an application installed, if it installed one.
///
/// A plugin reads it with `brocanth_openapi::document_of(lc.services())`.
/// The core has no `openapi()` accessor to offer, because the core has no
/// OpenAPI.
#[must_use]
pub fn document_of(services: &Services) -> Option<&Document> {
    services.get::<SharedDocument>().map(AsRef::as_ref)
}

/// What [`OpenApiPlugin`] is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenApiConfig {
    /// The document to serve.
    pub document: Document,
    /// Where the JSON lives.
    pub json_route: String,
    /// Where the YAML lives.
    pub yaml_route: String,
}

impl OpenApiConfig {
    /// Serves a document at `/openapi.json` and `/openapi.yaml`.
    #[must_use]
    pub fn new(document: Document) -> Self {
        Self {
            document,
            json_route: "/openapi.json".to_owned(),
            yaml_route: "/openapi.yaml".to_owned(),
        }
    }

    /// Serves the JSON somewhere else.
    #[must_use]
    pub fn json_route(mut self, path: impl Into<String>) -> Self {
        self.json_route = path.into();
        self
    }

    /// Serves the YAML somewhere else.
    #[must_use]
    pub fn yaml_route(mut self, path: impl Into<String>) -> Self {
        self.yaml_route = path.into();
        self
    }
}

/// Serves the document over HTTP.
#[derive(Debug)]
pub struct OpenApiPlugin {
    document: SharedDocument,
    json_route: String,
    yaml_route: String,
    json_body: String,
    yaml_body: String,
}

impl OpenApiPlugin {
    /// The document being served.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// A handle to the document that outlives a borrow of the plugin.
    #[must_use]
    pub fn shared(&self) -> SharedDocument {
        Arc::clone(&self.document)
    }

    /// The rendered body for one format.
    #[must_use]
    pub fn body(&self, format: Format) -> &str {
        match format {
            Format::Json => &self.json_body,
            Format::Yaml => &self.yaml_body,
        }
    }

    /// The format served at `path`, or [`None`] when the path is not ours.
    #[must_use]
    pub fn format_at(&self, path: &str) -> Option<Format> {
        if path == self.json_route {
            Some(Format::Json)
        } else if path == self.yaml_route {
            Some(Format::Yaml)
        } else {
            None
        }
    }

    /// The two routes this plugin answers.
    #[must_use]
    pub fn routes(&self) -> [(&str, Verb, Format); 2] {
        [
            (&self.json_route, Verb::Get, Format::Json),
            (&self.yaml_route, Verb::Get, Format::Yaml),
        ]
    }

    /// The response for one format.
    ///
    /// `Cache-Control: no-cache` rather than a long max-age: the document
    /// changes when the application is redeployed, and a client holding a
    /// stale one is the exact failure this crate exists to prevent.
    #[must_use]
    pub fn respond(&self, format: Format) -> http::Response<Vec<u8>> {
        let body = self.body(format).as_bytes().to_vec();
        http::Response::builder()
            .status(http::StatusCode::OK)
            .header(http::header::CONTENT_TYPE, format.media_type())
            .header(http::header::CACHE_CONTROL, "no-cache")
            .header(http::header::CONTENT_LENGTH, body.len())
            .body(body)
            .expect("every header here is a literal this crate controls")
    }

    /// The response for a path, or [`None`] when the path is not ours.
    #[must_use]
    pub fn serve(&self, path: &str) -> Option<http::Response<Vec<u8>>> {
        self.format_at(path).map(|format| self.respond(format))
    }

    /// Registers both routes in the application's router.
    ///
    /// `make` turns a [`Format`] into whatever the application's handler type
    /// is, which is how a plugin adds a route without the core knowing what a
    /// handler is.
    ///
    /// # Errors
    ///
    /// [`BuildError`] when one of the paths is already taken, which is the
    /// right answer: silently replacing an application's route would be the
    /// autowiring this framework refuses to do.
    pub fn mount<H, F>(&self, builder: &mut RouterBuilder<H>, mut make: F) -> Result<(), BuildError>
    where
        F: FnMut(Format) -> H,
    {
        for (path, verb, format) in self.routes() {
            builder.route(path, verb, make(format))?;
        }
        Ok(())
    }
}

impl Plugin for OpenApiPlugin {
    type Config = OpenApiConfig;
    type Provides = NoContext;
    const NAME: &'static str = "openapi";

    fn new(config: OpenApiConfig) -> Self {
        let json_body = render(&config.document, Format::Json);
        let yaml_body = render(&config.document, Format::Yaml);
        Self {
            document: Arc::new(config.document),
            json_route: config.json_route,
            yaml_route: config.yaml_route,
            json_body,
            yaml_body,
        }
    }

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // An invalid document is a build failure, not a 500 somebody finds
        // later. `configure` is the first hook of the first stage, so this
        // runs before any plugin opens a socket.
        self.document
            .validate()
            .map_err(|error| PluginError::new(error.to_string()))?;
        for path in [&self.json_route, &self.yaml_route] {
            if !path.starts_with('/') {
                return Err(PluginError::new(format!(
                    "the openapi route `{path}` does not start with `/`"
                )));
            }
        }
        if self.json_route == self.yaml_route {
            return Err(PluginError::new(format!(
                "the JSON and YAML documents cannot both be served at `{}`",
                self.json_route
            )));
        }
        Ok(())
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(Arc::clone(&self.document));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use brocanth::plugin_app::AppBuilder;
    use brocanth::routing::Router;

    use super::*;
    use crate::document::{Operation, PathItem};

    fn document() -> Document {
        let mut doc = Document::new("Blog", "1.0.0");
        let mut item = PathItem::default();
        item.operations
            .push((Verb::Get, Operation::new("get_posts")));
        doc.paths.push(("/posts".to_owned(), item));
        doc
    }

    #[test]
    fn the_plugin_serves_both_formats() {
        let plugin = OpenApiPlugin::new(OpenApiConfig::new(document()));
        let response = plugin.serve("/openapi.json").unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(
            response.headers()[http::header::CONTENT_TYPE],
            "application/json"
        );
        assert!(plugin.serve("/openapi.yaml").is_some());
        assert!(plugin.serve("/elsewhere").is_none());
    }

    #[test]
    fn the_document_reaches_the_services_store() {
        let app = AppBuilder::new()
            .plugin(OpenApiPlugin::new(OpenApiConfig::new(document())))
            .build()
            .unwrap();
        let shared = document_of(app.services()).expect("registered");
        assert_eq!(shared.info.title, "Blog");
    }

    #[test]
    fn an_invalid_document_fails_the_build() {
        let mut doc = document();
        let mut item = PathItem::default();
        item.operations
            .push((Verb::Get, Operation::new("get_posts")));
        doc.paths.push(("/other".to_owned(), item));
        let error = AppBuilder::new()
            .plugin(OpenApiPlugin::new(OpenApiConfig::new(doc)))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("operationId"));
    }

    #[test]
    fn mounting_uses_the_applications_own_handler_type() {
        let plugin = OpenApiPlugin::new(OpenApiConfig::new(document()));
        let mut builder: RouterBuilder<Format> = Router::builder();
        plugin.mount(&mut builder, |format| format).unwrap();
        let router = builder.build().unwrap();
        let matched = router.at("/openapi.yaml").unwrap();
        assert_eq!(matched.handler(Verb::Get), Some(&Format::Yaml));
    }
}
