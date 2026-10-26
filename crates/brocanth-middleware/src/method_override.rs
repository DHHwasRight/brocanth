//! A `DELETE` sent from an HTML form.
//!
//! A browser form can send `GET` and `POST` and nothing else. Rails spells
//! the way around that `_method`, Gorilla spells it
//! `HTTPMethodOverrideHandler`, and both mean: read the real verb out of
//! the POST and put it on the request before anything routes it.
//!
//! Two rules, both from Gorilla and both load-bearing:
//!
//! 1. **Only a `POST` may be overridden.** A `GET` that could turn into a
//!    `DELETE` is a link that deletes, which is what a crawler walks into.
//! 2. **Only to an unsafe verb.** Overriding a POST into a GET would let a
//!    form reach a read route that was never meant to take a body.
//!
//! The original request's verb stays available as [`Overridden`], because
//! a log that says `DELETE` and never says the wire said `POST` is a log
//! that cannot be matched against a proxy's.

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState,
};

use crate::{form_field, text};

/// The header Gorilla reads, and the one every client library sends.
const HEADER: &str = "x-http-method-override";

/// What the request said before it was overridden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Overridden {
    /// Always `POST` today, and named rather than assumed.
    pub was: http::Method,
    /// Where the new verb came from: the header, or the form field.
    pub from: Source,
}

/// Which spelling asked for the override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `X-HTTP-Method-Override`.
    Header,
    /// A field in an `application/x-www-form-urlencoded` body.
    Field,
}

/// Which spellings to honour.
#[derive(Clone, Debug)]
pub struct MethodOverrideConfig {
    /// Read `X-HTTP-Method-Override`.
    pub header: bool,
    /// Read this form field out of a urlencoded body. `None` reads none.
    ///
    /// Reading the field means parsing the body in a hook, which is cheap
    /// for a form and wrong for an upload, so a handler that takes
    /// multipart is never touched: only `application/x-www-form-urlencoded`
    /// is read.
    pub field: Option<String>,
}

impl Default for MethodOverrideConfig {
    /// Both spellings, with Rails's field name.
    fn default() -> Self {
        Self {
            header: true,
            field: Some("_method".to_string()),
        }
    }
}

/// Turns a POST into the verb it says it is.
pub struct MethodOverride {
    config: MethodOverrideConfig,
}

impl Plugin for MethodOverride {
    type Config = MethodOverrideConfig;
    type Provides = NoContext;
    const NAME: &'static str = "method-override";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for MethodOverride {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "rewrite",
            // Before anything that reads the verb, which is everything.
            brocanth::plugin_order::Order::new(),
            Hook {
                config: self.config.clone(),
            },
        );
        Ok(())
    }
}

struct Hook {
    config: MethodOverrideConfig,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if request.method() != http::Method::POST {
            return Ok(Flow::Continue);
        }
        let found = self
            .config
            .header
            .then(|| from_header(request))
            .flatten()
            .map(|verb| (verb, Source::Header))
            .or_else(|| {
                let field = self.config.field.as_deref()?;
                from_field(request, field).map(|verb| (verb, Source::Field))
            });
        let Some((verb, from)) = found else {
            return Ok(Flow::Continue);
        };
        if !unsafe_verb(&verb) {
            // A POST that asks to become a GET is asking to reach a read
            // route with a body. Gorilla refuses it and so does this.
            return Ok(Flow::Continue);
        }
        let was = std::mem::replace(request.method_mut(), verb);
        state.insert(Overridden { was, from });
        Ok(Flow::Continue)
    }
}

fn from_header(request: &Request) -> Option<http::Method> {
    let raw = text(
        request.headers(),
        http::header::HeaderName::from_static(HEADER),
    )?;
    raw.trim().to_ascii_uppercase().parse().ok()
}

fn from_field(request: &Request, field: &str) -> Option<http::Method> {
    let raw = form_field(request, field)?;
    raw.trim().to_ascii_uppercase().parse().ok()
}

/// The verbs a form is allowed to become.
fn unsafe_verb(verb: &http::Method) -> bool {
    matches!(
        *verb,
        http::Method::PUT | http::Method::PATCH | http::Method::DELETE
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post(headers: &[(&'static str, &str)], body: &str) -> Request {
        let mut builder = http::Request::builder().method(http::Method::POST);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        builder.body(body.as_bytes().to_vec()).expect("a request")
    }

    fn run(mut request: Request) -> (Request, RequestState) {
        let hook = Hook {
            config: MethodOverrideConfig::default(),
        };
        let mut state = RequestState::new();
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        (request, state)
    }

    #[test]
    fn the_header_renames_the_verb() {
        let (request, state) = run(post(&[("x-http-method-override", "DELETE")], ""));
        assert_eq!(request.method(), http::Method::DELETE);
        let was = state.get::<Overridden>().expect("recorded");
        assert_eq!(was.was, http::Method::POST, "the wire's verb is kept");
        assert_eq!(was.from, Source::Header);
    }

    #[test]
    fn a_form_field_renames_the_verb() {
        let (request, state) = run(post(
            &[("content-type", "application/x-www-form-urlencoded")],
            "title=Hi&_method=patch&body=x",
        ));
        assert_eq!(
            request.method(),
            http::Method::PATCH,
            "the value is upper-cased, because a form sends what it sends"
        );
        assert_eq!(
            state.get::<Overridden>().expect("recorded").from,
            Source::Field
        );
    }

    #[test]
    fn the_header_wins_over_the_field() {
        let (request, _) = run(post(
            &[
                ("x-http-method-override", "PUT"),
                ("content-type", "application/x-www-form-urlencoded"),
            ],
            "_method=DELETE",
        ));
        assert_eq!(request.method(), http::Method::PUT);
    }

    #[test]
    fn a_get_is_never_overridden() {
        // A link that deletes is a link a crawler walks into.
        let mut request = http::Request::builder()
            .method(http::Method::GET)
            .header("x-http-method-override", "DELETE")
            .body(Vec::new())
            .expect("a request");
        let hook = Hook {
            config: MethodOverrideConfig::default(),
        };
        let mut state = RequestState::new();
        hook.before_dispatch(&mut request, &mut state).expect("ok");
        assert_eq!(request.method(), http::Method::GET);
        assert!(state.get::<Overridden>().is_none());
    }

    #[test]
    fn a_post_may_not_become_a_safe_verb() {
        for verb in ["GET", "HEAD", "OPTIONS", "TRACE"] {
            let (request, state) = run(post(&[("x-http-method-override", verb)], ""));
            assert_eq!(
                request.method(),
                http::Method::POST,
                "a POST became {verb}, which reaches a read route with a body"
            );
            assert!(state.get::<Overridden>().is_none());
        }
    }

    #[test]
    fn a_multipart_body_is_never_parsed() {
        // Reading a field out of an upload would mean buffering and
        // scanning one, in a hook, on every request.
        let (request, _) = run(post(
            &[("content-type", "multipart/form-data; boundary=xyz")],
            "_method=DELETE",
        ));
        assert_eq!(request.method(), http::Method::POST);
    }

    #[test]
    fn a_charset_does_not_change_the_media_type() {
        let (request, _) = run(post(
            &[(
                "content-type",
                "application/x-www-form-urlencoded; charset=utf-8",
            )],
            "_method=DELETE",
        ));
        assert_eq!(request.method(), http::Method::DELETE);
    }

    #[test]
    fn nonsense_is_ignored_rather_than_refused() {
        // The request is still a valid POST. Refusing it would turn a
        // typo in somebody's client into a 400 nobody can read.
        let (request, _) = run(post(&[("x-http-method-override", "BANANA")], ""));
        assert_eq!(request.method(), http::Method::POST);
    }

    #[test]
    fn a_percent_encoded_field_still_matches() {
        assert_eq!(crate::decode("_method"), "_method");
        assert_eq!(crate::decode("a+b"), "a b");
        assert_eq!(crate::decode("%44ELETE"), "DELETE");
        let (request, _) = run(post(
            &[("content-type", "application/x-www-form-urlencoded")],
            "_method=%44elete",
        ));
        assert_eq!(request.method(), http::Method::DELETE);
    }
}
