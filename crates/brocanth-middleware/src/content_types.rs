//! 415 before a handler parses nonsense.
//!
//! Gorilla's `ContentTypeHandler` takes a list of media types and answers
//! `415 Unsupported Media Type` to any request whose body is something
//! else. It is a small thing that removes a whole class of handler code:
//! the branch where a JSON endpoint is sent a form and produces a parse
//! error that reads like a bug in the client's data rather than in its
//! headers.
//!
//! Only a request with a body is checked. A `GET` has nothing to be the
//! wrong type, and refusing one for its `Content-Type` would break every
//! client that sends a stale header on a verb that ignores it.

use brocanth::plugin_api::{Plugin, PluginError};
use brocanth::plugin_body::Body;
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    Flow, HookRegistrar, HookSource, OnRequest, Request, RequestHook, RequestState, Response,
};

use crate::text;

/// Which media types a body may be.
#[derive(Clone, Debug)]
pub struct ContentTypesConfig {
    /// The media types, without parameters: `application/json`.
    pub allowed: Vec<String>,
}

impl ContentTypesConfig {
    pub fn allowing(types: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            allowed: types
                .into_iter()
                .map(|t| t.into().trim().to_ascii_lowercase())
                .collect(),
        }
    }

    /// What a JSON API takes.
    pub fn json() -> Self {
        Self::allowing(["application/json"])
    }

    /// What an HTML application takes.
    pub fn forms() -> Self {
        Self::allowing(["application/x-www-form-urlencoded", "multipart/form-data"])
    }
}

/// Refuses a body that is not on the list.
pub struct ContentTypes {
    config: ContentTypesConfig,
}

impl Plugin for ContentTypes {
    type Config = ContentTypesConfig;
    type Provides = NoContext;
    const NAME: &'static str = "content-types";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Self::Config) -> Self {
        Self { config }
    }
}

impl HookSource for ContentTypes {
    type Families = OnRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        registrar.request(
            "check",
            brocanth::plugin_order::Order::new(),
            Hook {
                allowed: self.config.allowed.clone(),
            },
        );
        Ok(())
    }
}

struct Hook {
    allowed: Vec<String>,
}

impl RequestHook for Hook {
    fn before_dispatch(
        &self,
        request: &mut Request,
        _state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        if !carries_a_body(request) {
            return Ok(Flow::Continue);
        }
        let sent = text(request.headers(), http::header::CONTENT_TYPE).map(media_type);
        let ok = match sent {
            Some(kind) => self.allowed.iter().any(|a| a == &kind),
            // A body with no type. Gorilla refuses this, and so does
            // this: a handler that guesses is a handler that guesses
            // wrong on the one request that mattered.
            None => false,
        };
        if ok {
            return Ok(Flow::Continue);
        }
        let mut response = Response::new(Body::from(
            format!("this endpoint takes {}\n", self.allowed.join(", ")).into_bytes(),
        ));
        *response.status_mut() = http::StatusCode::UNSUPPORTED_MEDIA_TYPE;
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/plain; charset=utf-8"),
        );
        // RFC 9110 §15.5.16 does not define a header for this, so the
        // list goes in the body where a person reads it.
        Ok(Flow::respond(response))
    }
}

/// Whether this request has a body to have a type.
///
/// A non-empty body, or a verb that is defined to carry one. Both,
/// because a `POST` with an empty body is still a POST whose type is
/// wrong, and a `DELETE` with a JSON body is still carrying JSON.
fn carries_a_body(request: &Request) -> bool {
    if !request.body().is_empty() {
        return true;
    }
    matches!(
        *request.method(),
        http::Method::POST | http::Method::PUT | http::Method::PATCH
    )
}

/// `application/json; charset=utf-8` is `application/json`.
fn media_type(raw: &str) -> String {
    raw.split(';')
        .next()
        .unwrap_or(raw)
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hook() -> Hook {
        Hook {
            allowed: ContentTypesConfig::json().allowed,
        }
    }

    fn request(method: http::Method, kind: Option<&str>, body: &str) -> Request {
        let mut builder = http::Request::builder().method(method);
        if let Some(kind) = kind {
            builder = builder.header("content-type", kind);
        }
        builder.body(body.as_bytes().to_vec()).expect("a request")
    }

    fn status(mut request: Request) -> Option<http::StatusCode> {
        match hook()
            .before_dispatch(&mut request, &mut RequestState::new())
            .expect("ok")
        {
            Flow::Continue => None,
            Flow::Respond(response) => Some(response.status()),
        }
    }

    #[test]
    fn the_right_type_passes() {
        assert_eq!(
            status(request(http::Method::POST, Some("application/json"), "{}")),
            None
        );
    }

    #[test]
    fn a_charset_does_not_change_the_type() {
        assert_eq!(
            status(request(
                http::Method::POST,
                Some("application/json; charset=utf-8"),
                "{}"
            )),
            None
        );
        assert_eq!(
            media_type("Application/JSON ; charset=utf-8"),
            "application/json"
        );
    }

    #[test]
    fn the_wrong_type_is_415() {
        assert_eq!(
            status(request(
                http::Method::POST,
                Some("application/x-www-form-urlencoded"),
                "a=1"
            )),
            Some(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
    }

    #[test]
    fn a_body_with_no_type_is_415() {
        // A handler that guesses guesses wrong on the one request that
        // mattered.
        assert_eq!(
            status(request(http::Method::POST, None, "{}")),
            Some(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
    }

    #[test]
    fn a_get_is_never_checked() {
        // A stale Content-Type on a verb that ignores bodies is not a
        // reason to refuse the request.
        assert_eq!(
            status(request(http::Method::GET, Some("text/plain"), "")),
            None
        );
    }

    #[test]
    fn an_empty_post_is_still_checked() {
        assert_eq!(
            status(request(http::Method::POST, Some("text/plain"), "")),
            Some(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
    }

    #[test]
    fn a_delete_with_a_body_is_checked_and_an_empty_one_is_not() {
        assert_eq!(
            status(request(http::Method::DELETE, Some("text/plain"), "x")),
            Some(http::StatusCode::UNSUPPORTED_MEDIA_TYPE)
        );
        assert_eq!(status(request(http::Method::DELETE, None, "")), None);
    }

    #[test]
    fn the_answer_says_what_would_have_worked() {
        let mut request = request(http::Method::POST, Some("text/plain"), "x");
        let Flow::Respond(response) = hook()
            .before_dispatch(&mut request, &mut RequestState::new())
            .expect("ok")
        else {
            panic!("it passed");
        };
        let body = match response.into_body() {
            Body::Buffered(bytes) => String::from_utf8(bytes).expect("utf-8"),
            _ => panic!("a refusal is not a stream"),
        };
        assert!(body.contains("application/json"), "{body}");
    }
}
