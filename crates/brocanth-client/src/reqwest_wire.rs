//! The shipped sender.
//!
//! One blocking request and then the process exits, which is what a
//! command line does. `Wire` is the contract, so an application holding
//! its own client implements that instead and this module is never
//! compiled.

use brocanth::routing::Verb;

use crate::{Answer, Call, Wire};

/// Send with `reqwest`, blocking.
pub struct Reqwest {
    client: reqwest::blocking::Client,
}

impl Reqwest {
    /// A sender with the defaults.
    ///
    /// # Errors
    ///
    /// Anything that stops a client being built, which is a TLS backend
    /// that will not start.
    pub fn new() -> Result<Reqwest, String> {
        reqwest::blocking::Client::builder()
            .build()
            .map(|client| Reqwest { client })
            .map_err(|e| format!("the HTTP client would not start: {e}"))
    }
}

impl Wire for Reqwest {
    fn send(&self, call: &Call) -> Result<Answer, String> {
        let method = match call.verb {
            Verb::Get => reqwest::Method::GET,
            Verb::Head => reqwest::Method::HEAD,
            Verb::Post => reqwest::Method::POST,
            Verb::Put => reqwest::Method::PUT,
            Verb::Patch => reqwest::Method::PATCH,
            Verb::Delete => reqwest::Method::DELETE,
            Verb::Options => reqwest::Method::OPTIONS,
        };
        let mut request = self.client.request(method, &call.url);
        for (name, value) in &call.headers {
            request = request.header(name, value);
        }
        if let Some(body) = &call.body {
            request = request.body(body.clone());
        }

        let answered = request
            .send()
            .map_err(|e| format!("{} {}: {e}", call.verb.as_str(), call.url))?;
        let status = answered.status().as_u16();
        let content_type = answered
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = answered
            .text()
            .map_err(|e| format!("reading the answer: {e}"))?;

        Ok(Answer {
            status,
            body,
            content_type,
        })
    }
}
