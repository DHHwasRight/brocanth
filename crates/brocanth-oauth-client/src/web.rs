//! The connect pages, as a function from a request to an answer, with no
//! web framework assumed.
//!
//! [`ConnectRoutes::answer`] takes the parts of a request it reads
//! ([`ConnectRequest`]) and returns what to send ([`ConnectAnswer`]). The
//! application turns one into the other in a few lines, and renders a
//! [`ConnectPage`] with [`ConnectPage::html`] or its own templates.
//!
//! # The routes, under a prefix such as `/connections`
//!
//! | Route | Does |
//! |---|---|
//! | `GET /connections` | the signed-in person's connections, with a button each |
//! | `GET /connections/{provider}` | a page with one button, for a plain link |
//! | `POST /connections/{provider}` | a sealed flow cookie, then 303 to the provider |
//! | `GET /connections/{provider}/callback` | the provider's answer: the token is stored |
//! | `POST /connections/{provider}/disconnect` | revokes at the provider and deletes the row |
//!
//! Every route needs a signed-in account but the callback, which reads the
//! account from the flow cookie. The application says who is signed in
//! ([`ConnectRequest::account`]); this module does not read a session.
//!
//! # Decisions
//!
//! * **The flow starts with a POST.** A GET that starts a connection can be
//!   fired by any page on the web. A POST goes through the CSRF check.
//! * **The flow state is a sealed cookie, not a row.** A row per started
//!   flow is a write a stranger can cause. The cookie is `HttpOnly`,
//!   `SameSite=Lax` (the callback is a cross-site navigation), ten minutes.
//! * **The account rides in the cookie.** A `SameSite=Strict` session
//!   cookie is not sent on the cross-site callback, so the callback cannot
//!   read the session. The account was read from the session at the POST,
//!   and the seal keeps anybody from changing it. When the callback does
//!   carry a session, it must be the same account.
//! * **The callback answers a page, not a redirect.** A redirect that
//!   follows a cross-site navigation still carries no `Strict` cookie, so
//!   the person would land signed out. The page moves on by itself, from
//!   this site.
//! * **`return_to` is a local path or nothing.** An absolute URL, `//host`
//!   or `/\host` is dropped: no open redirect.

use crate::client::local_path;
use crate::{Error, FlowError, OauthClient};

/// The parts of a request the routes read.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConnectRequest<'a> {
    /// Whether the method is POST. Anything else is read as GET.
    pub post: bool,
    /// The path, with the prefix: `/connections/github/callback`.
    pub path: &'a str,
    /// The query string, without the `?`.
    pub query: &'a str,
    /// The body of a POST: an urlencoded form.
    pub body: &'a str,
    /// The `Cookie` header, or `""`.
    pub cookies: &'a str,
    /// The signed-in account, or `None`. The application reads it from its
    /// session; an impersonated session should give `None`.
    pub account: Option<&'a str>,
}

/// What to send back.
#[derive(Debug)]
pub enum ConnectAnswer {
    /// `303 See Other` to `location`, with these `Set-Cookie` values.
    Redirect {
        /// Where to.
        location: String,
        /// `Set-Cookie` values.
        cookies: Vec<String>,
    },
    /// A page, with this status and these `Set-Cookie` values.
    Page {
        /// The HTTP status.
        status: u16,
        /// What to say.
        page: ConnectPage,
        /// `Set-Cookie` values.
        cookies: Vec<String>,
    },
}

impl ConnectAnswer {
    fn page(status: u16, page: ConnectPage) -> Self {
        Self::Page {
            status,
            page,
            cookies: Vec::new(),
        }
    }
}

/// One row of the connections page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderRow {
    /// `github`.
    pub id: String,
    /// `GitHub`.
    pub name: String,
    /// Whether the account has a token for it.
    pub connected: bool,
    /// Where the Connect button posts.
    pub connect: String,
    /// Where the Disconnect button posts.
    pub disconnect: String,
}

/// A page the routes answer. Every string is plain text; [`ConnectPage::html`]
/// escapes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectPage {
    /// The signed-in person's connections.
    List {
        /// One row per provider.
        rows: Vec<ProviderRow>,
    },
    /// One button that starts a connection.
    Continue {
        /// `GitHub`.
        provider: String,
        /// Where the button posts.
        action: String,
    },
    /// Connected: go on to `location`.
    Connected {
        /// `GitHub`.
        provider: String,
        /// Where to go.
        location: String,
    },
    /// Nobody is signed in.
    SignIn,
    /// The flow failed. The message is safe to show.
    Refused {
        /// What went wrong.
        message: String,
    },
    /// No provider has that id, or no route has that path.
    NotFound,
}

impl ConnectPage {
    /// The page as a small HTML document. Replace it with your own
    /// templates by matching on the variants.
    pub fn html(&self) -> String {
        let (title, body) = match self {
            ConnectPage::List { rows } => {
                let mut body = String::from("<ul>");
                for row in rows {
                    let (label, action, button) = if row.connected {
                        ("connected", &row.disconnect, "Disconnect")
                    } else {
                        ("not connected", &row.connect, "Connect")
                    };
                    body.push_str(&format!(
                        "<li>{} ({label}) <form method=\"post\" action=\"{}\"><button type=\"submit\">{button}</button></form></li>",
                        escape(&row.name),
                        escape(action),
                    ));
                }
                body.push_str("</ul>");
                ("Connected accounts".to_string(), body)
            }
            ConnectPage::Continue { provider, action } => (
                format!("Connect {}", escape(provider)),
                format!(
                    "<form method=\"post\" action=\"{}\"><button type=\"submit\">Connect {}</button></form>",
                    escape(action),
                    escape(provider)
                ),
            ),
            ConnectPage::Connected { provider, location } => (
                format!("{} connected", escape(provider)),
                format!(
                    "<meta http-equiv=\"refresh\" content=\"0;url={0}\"><p>{1} is connected. <a href=\"{0}\">Continue</a></p>",
                    escape(location),
                    escape(provider)
                ),
            ),
            ConnectPage::SignIn => (
                "Sign in first".to_string(),
                "<p>Sign in to connect an account.</p>".to_string(),
            ),
            ConnectPage::Refused { message } => (
                "Connection failed".to_string(),
                format!("<p>{}</p>", escape(message)),
            ),
            ConnectPage::NotFound => ("Not found".to_string(), "<p>Not found.</p>".to_string()),
        };
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{title}</title></head><body><h1>{title}</h1>{body}</body></html>\n"
        )
    }
}

/// The connect routes over one [`OauthClient`].
#[derive(Clone, Debug)]
pub struct ConnectRoutes {
    client: OauthClient,
    prefix: String,
    cookie: String,
    secure: bool,
}

impl ConnectRoutes {
    /// The routes at `prefix` (`/connections`, no trailing slash). `secure`
    /// adds `Secure` to the flow cookie and names it `__Host-oauth_client`;
    /// turn it on whenever the site is served over https.
    pub fn new(client: OauthClient, prefix: &str, secure: bool) -> Self {
        Self {
            client,
            prefix: prefix.trim_end_matches('/').to_string(),
            cookie: if secure {
                "__Host-oauth_client".to_string()
            } else {
                "oauth_client".to_string()
            },
            secure,
        }
    }

    /// The flow cookie's name.
    pub fn cookie_name(&self) -> &str {
        &self.cookie
    }

    /// Answers one request under the prefix.
    ///
    /// # Errors
    /// The store failed, or the provider's token endpoint did. A refused
    /// flow is a page, not an error.
    pub async fn answer(&self, request: ConnectRequest<'_>) -> Result<ConnectAnswer, Error> {
        let Some(rest) = request.path.strip_prefix(self.prefix.as_str()) else {
            return Ok(ConnectAnswer::page(404, ConnectPage::NotFound));
        };
        let rest = rest.trim_matches('/');
        if rest.is_empty() {
            return self.list(request).await;
        }
        let (id, step) = rest.split_once('/').unwrap_or((rest, ""));
        let Ok(provider) = self.client.provider(id) else {
            return Ok(ConnectAnswer::page(404, ConnectPage::NotFound));
        };
        let name = provider.name.clone();
        if step == "callback" {
            return self.callback(id, &name, request).await;
        }
        let Some(account) = request.account else {
            return Ok(ConnectAnswer::page(401, ConnectPage::SignIn));
        };
        match (step, request.post) {
            ("", false) => Ok(ConnectAnswer::page(
                200,
                ConnectPage::Continue {
                    provider: name,
                    action: format!("{}/{id}", self.prefix),
                },
            )),
            ("", true) => {
                let form = pairs(request.body);
                let return_to = form
                    .iter()
                    .find(|(k, _)| k == "return_to")
                    .map(|(_, v)| v.as_str());
                let started = self.client.begin(id, account, return_to)?;
                Ok(ConnectAnswer::Redirect {
                    location: started.url,
                    cookies: vec![self.set_cookie(&started.cookie, 600)],
                })
            }
            ("disconnect", true) => {
                self.client.disconnect(account, id).await?;
                Ok(ConnectAnswer::Redirect {
                    location: self.prefix.clone(),
                    cookies: Vec::new(),
                })
            }
            _ => Ok(ConnectAnswer::page(404, ConnectPage::NotFound)),
        }
    }

    async fn list(&self, request: ConnectRequest<'_>) -> Result<ConnectAnswer, Error> {
        if request.post {
            return Ok(ConnectAnswer::page(404, ConnectPage::NotFound));
        }
        let Some(account) = request.account else {
            return Ok(ConnectAnswer::page(401, ConnectPage::SignIn));
        };
        let connected = self.client.connections(account).await?;
        let rows = self
            .client
            .providers()
            .map(|p| ProviderRow {
                id: p.id.clone(),
                name: p.name.clone(),
                connected: connected.iter().any(|c| c.provider == p.id),
                connect: format!("{}/{}", self.prefix, p.id),
                disconnect: format!("{}/{}/disconnect", self.prefix, p.id),
            })
            .collect();
        Ok(ConnectAnswer::page(200, ConnectPage::List { rows }))
    }

    async fn callback(
        &self,
        id: &str,
        name: &str,
        request: ConnectRequest<'_>,
    ) -> Result<ConnectAnswer, Error> {
        let clear = self.set_cookie("", 0);
        let sealed = cookie(request.cookies, &self.cookie);
        let query = pairs(request.query);
        match self
            .client
            .complete(id, sealed, &query, request.account)
            .await
        {
            Ok(done) => Ok(ConnectAnswer::Page {
                status: 200,
                page: ConnectPage::Connected {
                    provider: name.to_string(),
                    location: done
                        .return_to
                        .and_then(|r| local_path(&r))
                        .unwrap_or_else(|| self.prefix.clone()),
                },
                cookies: vec![clear],
            }),
            Err(Error::Flow(flow)) => Ok(ConnectAnswer::Page {
                status: 400,
                page: ConnectPage::Refused {
                    message: sentence(&flow),
                },
                cookies: vec![clear],
            }),
            Err(Error::Provider { .. }) => Ok(ConnectAnswer::Page {
                status: 502,
                page: ConnectPage::Refused {
                    message: format!("{name} did not complete the connection. Start again."),
                },
                cookies: vec![clear],
            }),
            Err(other) => Err(other),
        }
    }

    fn set_cookie(&self, value: &str, max_age: i64) -> String {
        format!(
            "{}={value}; Max-Age={max_age}; Path=/; HttpOnly{}; SameSite=Lax",
            self.cookie,
            if self.secure { "; Secure" } else { "" }
        )
    }
}

/// The first line of a flow error, which is the part a visitor reads.
fn sentence(flow: &FlowError) -> String {
    let text = flow.to_string();
    let first = text.lines().next().unwrap_or_default();
    let mut chars = first.chars();
    match chars.next() {
        Some(c) => format!("{}{}.", c.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// One cookie's value from a `Cookie` header.
fn cookie<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.split(';').find_map(|pair| {
        let (k, v) = pair.trim().split_once('=')?;
        (k == name && !v.is_empty()).then_some(v)
    })
}

/// Urlencoded pairs, decoded.
fn pairs(raw: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(raw.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Escapes text for HTML, in an element or a quoted attribute.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cookie_is_read_by_its_exact_name() {
        let header = "a=1; oauth_client_x=2; oauth_client=3";
        assert_eq!(cookie(header, "oauth_client"), Some("3"));
        assert_eq!(cookie(header, "b"), None);
    }

    #[test]
    fn a_flow_error_becomes_one_sentence() {
        assert_eq!(
            sentence(&FlowError::Expired),
            "This connection took longer than ten minutes."
        );
    }

    #[test]
    fn a_page_escapes_what_it_prints() {
        let html = ConnectPage::Refused {
            message: "<script>".into(),
        }
        .html();
        assert!(html.contains("&lt;script&gt;") && !html.contains("<script>"));
    }
}
