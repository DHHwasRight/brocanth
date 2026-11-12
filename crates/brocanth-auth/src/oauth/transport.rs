//! `oauth2` and `openidconnect` send their requests through this crate's
//! [`HttpClient`].
//!
//! Both crates take any type that implements [`oauth2::AsyncHttpClient`].
//! [`Transport`] is that type over an [`HttpClient`], so the seam stays the
//! one an application already implements, and a test double such as
//! [`StubHttp`](crate::http_client::StubHttp) still sees every request. The
//! crates build the request; this file only moves it.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use http::header::CONTENT_TYPE;

use crate::error::AuthError;
use crate::http_client::{HttpClient, HttpRequest};

/// An [`HttpClient`] as an [`oauth2::AsyncHttpClient`].
pub(crate) struct Transport<H: HttpClient>(pub(crate) Arc<H>);

impl<'c, H: HttpClient> oauth2::AsyncHttpClient<'c> for Transport<H> {
    type Error = AuthError;
    type Future =
        Pin<Box<dyn Future<Output = Result<oauth2::HttpResponse, AuthError>> + Send + 'c>>;

    fn call(&'c self, request: oauth2::HttpRequest) -> Self::Future {
        Box::pin(async move {
            let response = self.0.send(from_crate(request)?).await?;
            http::Response::builder()
                .status(response.status)
                .body(response.body)
                .map_err(|e| AuthError::Http(format!("could not read the provider's answer: {e}")))
        })
    }
}

/// The crate's request as this crate's [`HttpRequest`].
///
/// The body the crates send is always a form (a token or refresh request)
/// or nothing (a key set). The form is decoded back into pairs, so an
/// [`HttpClient`] keeps one way to send it.
fn from_crate(request: oauth2::HttpRequest) -> Result<HttpRequest, AuthError> {
    let (parts, body) = request.into_parts();
    let mut out = if parts.method == http::Method::POST {
        let form = url::form_urlencoded::parse(&body)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        HttpRequest::post_form(parts.uri.to_string(), form)
    } else {
        HttpRequest::get(parts.uri.to_string())
    };
    for (name, value) in &parts.headers {
        // The form encoding sets its own content type.
        if name == CONTENT_TYPE {
            continue;
        }
        let value = value.to_str().map_err(|_| {
            AuthError::Http(format!(
                "the header `{name}` is not text and cannot be sent"
            ))
        })?;
        out = out.header(canonical(name.as_str()), value);
    }
    Ok(out)
}

/// `accept` as `Accept`: the spelling this crate's own requests use, so a
/// header set twice is one entry and not two.
fn canonical(name: &str) -> String {
    name.split('-')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_form_body_comes_back_as_pairs_and_the_headers_keep_their_values() {
        let request = http::Request::builder()
            .method(http::Method::POST)
            .uri("https://provider.test/token")
            .header("accept", "application/json")
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(b"grant_type=authorization_code&code=a%2Bb".to_vec())
            .unwrap();
        let out = from_crate(request).unwrap();
        assert_eq!(out.method, http::Method::POST);
        assert_eq!(
            out.form.unwrap(),
            vec![
                ("grant_type".to_string(), "authorization_code".to_string()),
                ("code".to_string(), "a+b".to_string()),
            ]
        );
        assert_eq!(
            out.headers.get("Accept").map(String::as_str),
            Some("application/json")
        );
        assert!(!out.headers.contains_key("Content-Type"));
    }
}
