//! What every route takes and gives.
//!
//! One table, written by `brocanth_pages::pages!("src/pages", describe)`
//! while the crate compiles, and read by everything that has to agree with
//! the handlers: an OpenAPI document, a command-line client, a form
//! builder.
//!
//! Nothing in it is written by hand, so nothing in it can be out of date.
//! A route is a file, a verb is a function, and the signature of that
//! function says the rest:
//!
//! ```ignore
//! pub fn handle_post(ctx: &mut Ctx, body: Signup) -> Result<Json<Created>, Error>
//! //                                      ^ takes              ^ gives
//! ```
//!
//! A page that writes `-> Result<impl IntoResponse, Error>` keeps working
//! and contributes no schema, which is why the fields are [`Option`]. The
//! wrapper is the opt-in.

use brocanth::routing::Verb;
use zod::Node;

/// One operation: a path, a verb, and the shapes on either side of it.
#[derive(Clone, Debug)]
pub struct Described {
    /// The route, as a document writes it: `/users/{id}`.
    pub path: &'static str,
    /// The verb, read off the handler's name.
    pub verb: Verb,
    /// What the body must be, for a handler that takes one.
    pub takes: Option<Node>,
    /// What the answer is, for a handler that says.
    pub gives: Option<Node>,
}

impl Described {
    /// A stable identifier for this operation.
    ///
    /// `get_users_by_id` for `GET /users/{id}`: the verb, then the path
    /// with its parameters named rather than punctuated. The same spelling
    /// `brocanth-openapi` gives an operation, so a document and a client
    /// agree on what to call things.
    pub fn id(&self) -> String {
        let mut out = self.verb.as_str().to_ascii_lowercase();
        let mut any = false;
        for segment in self.path.split('/').filter(|s| !s.is_empty()) {
            any = true;
            out.push('_');
            match parameter(segment) {
                Some(name) => {
                    out.push_str("by_");
                    out.push_str(&name.replace('-', "_"));
                }
                None => out.push_str(&segment.replace('-', "_")),
            }
        }
        if !any {
            out.push_str("_root");
        }
        out
    }

    /// The name a person types on a command line: `users-id get`.
    ///
    /// Hyphens rather than underscores, because that is what every other
    /// command-line tool uses and a client should not be the exception.
    pub fn command(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        for segment in self.path.split('/').filter(|s| !s.is_empty()) {
            parts.push(match parameter(segment) {
                Some(name) => name.replace('_', "-"),
                None => segment.replace('_', "-"),
            });
        }
        if parts.is_empty() {
            parts.push("root".to_string());
        }
        parts.push(self.verb.as_str().to_ascii_lowercase());
        parts.join("-")
    }

    /// Every parameter the path declares, in order.
    pub fn parameters(&self) -> Vec<&'static str> {
        self.path.split('/').filter_map(parameter).collect()
    }

    /// The path with its parameters filled in, or the name of the first
    /// one that was not given.
    ///
    /// # Errors
    ///
    /// The name of a parameter the caller did not supply. A URL with
    /// `{id}` still in it is a request to a route that does not exist, and
    /// finding that out from a 404 is worse than being told here.
    pub fn fill(&self, mut value: impl FnMut(&str) -> Option<String>) -> Result<String, String> {
        let mut out = String::new();
        for segment in self.path.split('/').skip(1) {
            out.push('/');
            match parameter(segment) {
                Some(name) => out.push_str(&value(name).ok_or_else(|| name.to_string())?),
                None => out.push_str(segment),
            }
        }
        if out.is_empty() {
            out.push('/');
        }
        Ok(out)
    }

    /// Whether this operation sends a body.
    pub fn sends_body(&self) -> bool {
        self.takes.is_some()
    }
}

/// `{name}` and `{*name}` are parameters; anything else is a literal.
fn parameter(segment: &str) -> Option<&str> {
    segment
        .strip_prefix('{')?
        .strip_suffix('}')
        .map(|name| name.strip_prefix('*').unwrap_or(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &'static str, verb: Verb) -> Described {
        Described {
            path,
            verb,
            takes: None,
            gives: None,
        }
    }

    #[test]
    fn an_operation_id_names_the_verb_and_the_path() {
        assert_eq!(at("/users/{id}", Verb::Get).id(), "get_users_by_id");
        assert_eq!(at("/posts", Verb::Post).id(), "post_posts");
        assert_eq!(at("/", Verb::Get).id(), "get_root");
    }

    #[test]
    fn a_command_is_what_a_person_types() {
        assert_eq!(at("/users/{id}", Verb::Delete).command(), "users-id-delete");
        assert_eq!(at("/", Verb::Get).command(), "root-get");
        assert_eq!(
            at("/api/v1/posts", Verb::Post).command(),
            "api-v1-posts-post"
        );
    }

    #[test]
    fn the_parameters_are_the_braced_segments_in_order() {
        let route = at("/orgs/{org}/users/{id}", Verb::Get);
        assert_eq!(route.parameters(), vec!["org", "id"]);
        assert!(at("/posts", Verb::Get).parameters().is_empty());
    }

    #[test]
    fn a_catch_all_is_a_parameter_without_its_star() {
        assert_eq!(at("/docs/{*rest}", Verb::Get).parameters(), vec!["rest"]);
    }

    #[test]
    fn filling_a_path_puts_the_values_where_the_braces_were() {
        let route = at("/orgs/{org}/users/{id}", Verb::Get);
        let url = route
            .fill(|name| match name {
                "org" => Some("acme".into()),
                "id" => Some("7".into()),
                _ => None,
            })
            .unwrap();
        assert_eq!(url, "/orgs/acme/users/7");
    }

    #[test]
    fn a_missing_parameter_is_named_rather_than_left_in_the_url() {
        let route = at("/users/{id}", Verb::Get);
        assert_eq!(route.fill(|_| None), Err("id".to_string()));
    }

    #[test]
    fn a_route_with_no_parameters_fills_to_itself() {
        assert_eq!(at("/posts", Verb::Get).fill(|_| None).unwrap(), "/posts");
    }
}
