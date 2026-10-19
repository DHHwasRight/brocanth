//! The brocanth plugin, which is how an application names the admin in `main`.
//!
//! PLAN §8 is the shape of this: the dependency arrow points from here to the
//! core, the core knows nothing about an admin, and an application that does
//! not write `app.plugin(AdminPlugin::new(..))` links none of this.
//!
//! The plugin publishes the built [`Admin`] into [`Services`] during
//! `register`, so a handler reaches it the way it reaches any other plugin's
//! value. It offers no middleware, because it needs none.
//!
//! The routes come from [`RouteSource`], so an application writes one line
//! and gets every screen:
//!
//! ```text
//! app.mount_routes::<AdminPlugin>("/admin")?;
//! ```
//!
//! Four routes and not twelve, which is the one place this crate differs
//! from what `plugin_routes` expects of a bundle. The admin's path grammar
//! lives in [`Route::parse`](crate::Route), a panel owns every path under
//! its own key to a depth the admin cannot know, and splitting that
//! grammar between the bundle and the parser would be two places to get it
//! wrong. So the bundle is `GET`/`POST` on `/` and on `/{*rest}`, and
//! `dispatch` reads the path as it always did.
//!
//! # Who is asking
//!
//! [`Admin::dispatch`] takes a [`Principal`], and this crate has no
//! opinion about sessions, cookies or tokens — PLAN §8 does not allow it
//! one. So [`AdminSetup::principal`] takes a closure, and the default
//! returns [`Principal::anonymous`], which the default [`DenyAll`] policy
//! refuses. An admin mounted and not told who is asking answers 403, which
//! is the same way it ships locked everywhere else.
//!
//! The closure reads a [`Request`] and a [`RequestState`], and neither is
//! generic over the provider list, which is what lets this plugin stay
//! un-parameterised. An application's auth middleware puts its user in the
//! request state; this reads it out.
//!
//! [`Services`]: brocanth::plugin_api::Services
//! [`DenyAll`]: crate::policy::DenyAll
//! [`Principal`]: crate::policy::Principal

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::HandlerError;
use brocanth::plugin_body::Body;
use brocanth::plugin_hooks::{Request, RequestState};
use brocanth::plugin_routes::{async_handler, AsyncHandler, RouteBundle, RouteCx, RouteSource};
use brocanth::routing_verb::Verb;

use crate::admin::{Admin, Response as AdminResponse};
use crate::config::AdminConfig;
use crate::data::SharedData;
use crate::form::Submission;
use crate::policy::Principal;

/// How an application says who is asking.
///
/// It is handed the request and the request state, and returns whatever
/// the policy will be asked about. Both arguments are free of the
/// type-level provider list, which is what keeps [`AdminPlugin`] from
/// having to be generic over it.
pub type PrincipalFn =
    Arc<dyn for<'a> Fn(&'a Request, &'a RequestState) -> Principal<'a> + Send + Sync>;

/// What the admin plugin is built from: a data source and a configuration.
pub struct AdminSetup {
    /// Where the rows come from.
    pub data: SharedData,
    /// How the admin is configured, including the policy and the views.
    pub config: AdminConfig,
    /// Who is asking. Anonymous until the application says otherwise.
    pub principal: PrincipalFn,
}

impl AdminSetup {
    /// A setup from a data source and a configuration.
    ///
    /// Nobody is asking until [`AdminSetup::principal`] says who, and the
    /// default [`DenyAll`](crate::policy::DenyAll) policy refuses nobody.
    /// Both halves have to be supplied before a screen answers 200, which
    /// is the point.
    pub fn new(data: SharedData, config: AdminConfig) -> Self {
        AdminSetup {
            data,
            config,
            principal: Arc::new(|_, _| Principal::anonymous()),
        }
    }

    /// How to find the identity the policy will be asked about.
    ///
    /// ```ignore
    /// AdminSetup::new(data, config).principal(|_req, state| {
    ///     state.get::<CurrentUser>().map_or_else(Principal::anonymous, Principal::new)
    /// })
    /// ```
    ///
    /// The value has to outlive the call, which is why it comes out of the
    /// request state rather than being built here: the admin borrows it
    /// and hands it to the policy, and never looks inside.
    #[must_use]
    pub fn principal<F>(mut self, f: F) -> Self
    where
        F: for<'a> Fn(&'a Request, &'a RequestState) -> Principal<'a> + Send + Sync + 'static,
    {
        self.principal = Arc::new(f);
        self
    }
}

/// The admin, as a brocanth plugin.
///
/// ```ignore
/// use std::sync::Arc;
///
/// use brocanth_admin::{AdminConfig, AdminPlugin, AdminSetup};
///
/// let app = App::builder()
///     .plugin(AdminPlugin::new(AdminSetup::new(
///         Arc::new(source),
///         AdminConfig::new().at("/admin").with_policy(my_policy),
///     )))
///     .build()?;
/// ```
pub struct AdminPlugin {
    admin: Arc<Admin>,
    principal: PrincipalFn,
}

impl AdminPlugin {
    /// The built admin, for an application that would rather hold it directly
    /// than reach it through [`Services`].
    ///
    /// [`Services`]: brocanth::plugin_api::Services
    pub fn admin(&self) -> Arc<Admin> {
        Arc::clone(&self.admin)
    }
}

impl Plugin for AdminPlugin {
    type Config = AdminSetup;
    type Provides = NoContext;

    const NAME: &'static str = "admin";

    const CONTRIBUTES_ROUTES: bool = true;

    fn new(config: AdminSetup) -> Self {
        AdminPlugin {
            admin: Arc::new(Admin::new(config.data, config.config)),
            principal: config.principal,
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        lc.services_mut().insert(Arc::clone(&self.admin));
        Ok(())
    }
}

impl<E: 'static> RouteSource<E> for AdminPlugin {
    const PREFIX: &'static str = "/admin";

    fn routes(&self, bundle: &mut RouteBundle<'_, E>) -> Result<(), PluginError> {
        // The index, and everything else. `{*rest}` matches zero or more
        // segments, so the first pair is not strictly needed -- it is here
        // so `brocanth routes` says the admin has an index, which is the
        // one thing a reader wants to know about a catch-all.
        bundle.described_async(
            "index",
            Verb::Get,
            "/",
            "the admin's front page: every model this person may list",
            self.screen(),
        );
        bundle.described_async(
            "screen",
            Verb::Get,
            "/{*rest}",
            "a list, a record, a form, or a tab another crate contributed",
            self.screen(),
        );
        bundle.described_async(
            "submit",
            Verb::Post,
            "/",
            "a write, for a screen mounted at the root",
            self.screen(),
        );
        bundle.described_async(
            "write",
            Verb::Post,
            "/{*rest}",
            "a create, an edit, a delete, or a panel's button",
            self.screen(),
        );
        Ok(())
    }
}

impl AdminPlugin {
    /// One handler, four times. `Admin::dispatch` owns the path grammar
    /// and gets the whole path, so there is nothing here to branch on.
    fn screen<E: 'static>(&self) -> AsyncHandler<E> {
        let admin = Arc::clone(&self.admin);
        let principal = Arc::clone(&self.principal);
        async_handler(move |cx: &mut RouteCx<'_, E>| {
            let admin = Arc::clone(&admin);
            let principal = Arc::clone(&principal);
            Box::pin(async move {
                let request = cx.request();
                let method = request.method().as_str().to_owned();
                let path = request.uri().path().to_owned();
                let query = request.uri().query().unwrap_or("").to_owned();
                // A write arrives as a form body. Anything else is a read
                // and the body is not looked at.
                let submission = submission_of(request);

                // Borrowed from the request and the state, which both
                // outlive the dispatch. That is why the closure returns a
                // `Principal<'a>` rather than an owned value: the admin
                // hands it to the policy and never looks inside.
                let who = principal(request, cx.state());

                let answer = admin
                    .dispatch(who, &method, &path, &query, &submission)
                    .await
                    .map_err(HandlerError::new)?;
                Ok(http_response(answer))
            })
        })
    }
}

/// The form body, as the admin reads one.
///
/// `application/x-www-form-urlencoded` and nothing else. Multipart is what
/// a file upload needs and the admin has no file widget yet; a request that
/// is multipart arrives here as an empty submission, so the form re-renders
/// with its errors rather than writing half a record.
fn submission_of(request: &Request) -> Submission {
    let urlencoded = request
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/x-www-form-urlencoded"));
    if !urlencoded {
        return Submission::new();
    }
    let body = std::str::from_utf8(request.body()).unwrap_or("");
    Submission::from_pairs(crate::query::pairs(body))
}

/// The admin's answer as the core's response.
fn http_response(answer: AdminResponse) -> brocanth::plugin_hooks::Response {
    match answer {
        AdminResponse::Page { status, html } => http::Response::builder()
            .status(status)
            .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(html))
            .expect("a status the admin produced is a valid status"),
        AdminResponse::Redirect { status, location } => http::Response::builder()
            .status(status)
            .header(http::header::LOCATION, location)
            .body(Body::empty())
            .expect("a redirect the admin produced is a valid response"),
    }
}
