//! Turning parsed arguments into one request.

use brocanth::routing::Verb;
use brocanth::routing_path::{encode_catch_all, encode_segment};
use brocanth_input::Described;
use clap::ArgMatches;
use zod::node::{Field, Node};
use zod::Value;

/// One request, ready to send.
///
/// A plain value, so the sending is somebody else's and a test can read
/// what would have gone out without a server to send it to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Call {
    /// The verb.
    pub verb: Verb,
    /// The whole URL, with the path filled in and the query on the end.
    pub url: String,
    /// Headers to send, in the order they were given.
    pub headers: Vec<(String, String)>,
    /// The body, for an operation that takes one.
    pub body: Option<String>,
}

/// Work out what the parsed arguments ask for.
///
/// # Errors
///
/// A message ready to print: an operation nobody matched, a path
/// parameter with no value, or a pair that is not `name=value`.
pub fn plan(routes: &[Described], matches: &ArgMatches) -> Result<Call, String> {
    let (name, sub) = matches.subcommand().ok_or_else(|| {
        "no operation was named\nhelp: run `--help` to list the operations".to_string()
    })?;
    let route = routes
        .iter()
        .find(|route| route.command() == name)
        .ok_or_else(|| {
            format!(
                "`{name}` is not an operation of this application\n\
                 help: run `--help` to list the operations"
            )
        })?;

    let base = matches
        .get_one::<String>("base")
        .map(|s| s.trim_end_matches('/').to_string())
        .unwrap_or_default();

    // Each value is percent-encoded, so `--id 'a/b?c'` stays one segment
    // and cannot add a path or a query. A catch-all keeps its slashes.
    let path = route
        .fill(|parameter| {
            let value = sub.get_one::<String>(&parameter.replace('_', "-"))?;
            Some(if route.path.contains(&format!("{{*{parameter}}}")) {
                encode_catch_all(value)
            } else {
                encode_segment(value)
            })
        })
        .map_err(|missing| {
            format!(
                "`--{missing}` is in the path and was not given\n\
                 help: write `--{missing} <VALUE>`"
            )
        })?;

    let mut url = format!("{base}{path}");
    let query = pairs(matches, "query", '=')?;
    if !query.is_empty() {
        let written: Vec<String> = query
            .iter()
            .map(|(name, value)| format!("{}={}", encode_segment(name), encode_segment(value)))
            .collect();
        url.push('?');
        url.push_str(&written.join("&"));
    }

    let mut headers = pairs(matches, "header", ':')?;
    let body = body_of(route, sub);
    if body.is_some()
        && !headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
    {
        headers.push(("content-type".into(), "application/json".into()));
    }

    Ok(Call {
        verb: route.verb,
        url,
        headers,
        body,
    })
}

/// The body: whole JSON if `--body` was given, otherwise the flags.
fn body_of(route: &Described, sub: &ArgMatches) -> Option<String> {
    if let Some(whole) = sub.try_get_one::<String>("body").ok().flatten() {
        return Some(whole.clone());
    }
    let object = route.takes.as_ref().and_then(Node::object)?;
    let mut fields: Vec<(String, Value)> = Vec::new();
    for field in &object.fields {
        // No flag of its own, so a value under its id is another flag's.
        if crate::clashes(route, field) {
            continue;
        }
        if let Some(value) = value_of(field, sub) {
            fields.push((field.name.to_string(), value));
        }
    }
    Some(zod::write(&Value::Map(fields)))
}

/// One field's value, or `None` when the flag was not given.
///
/// A flag nobody gave is a field nobody sends, which is not the same as a
/// field sent as null: the first lets a server default apply and the
/// second overrides it.
fn value_of(field: &Field, sub: &ArgMatches) -> Option<Value> {
    let name = field.name.replace('_', "-");
    match field.node.bare() {
        // A required boolean is a flag, so it is always sent: `SetTrue`
        // makes absent mean false rather than mean nothing.
        Node::Bool => sub
            .try_get_one::<bool>(&name)
            .ok()
            .flatten()
            .map(|b| Value::Bool(*b)),
        Node::Int(_) => sub
            .try_get_one::<i64>(&name)
            .ok()
            .flatten()
            .map(|n| Value::Int(*n)),
        Node::Real(_) => sub
            .try_get_one::<f64>(&name)
            .ok()
            .flatten()
            .map(|n| Value::Real(*n)),
        // Each item as the value parser `argument` gave it.
        Node::List { item, .. } => match item.bare() {
            Node::Int(_) => many(sub, &name, |n: &i64| Value::Int(*n)),
            Node::Real(_) => many(sub, &name, |n: &f64| Value::Real(*n)),
            Node::Bool => many(sub, &name, |b: &bool| Value::Bool(*b)),
            _ => many(sub, &name, |text: &String| Value::Text(text.clone())),
        },
        Node::Object(_) => None,
        _ => sub
            .try_get_one::<String>(&name)
            .ok()
            .flatten()
            .map(|text| Value::Text(text.clone())),
    }
}

/// Every value of a repeated flag, as JSON values.
fn many<T: Clone + Send + Sync + 'static>(
    sub: &ArgMatches,
    name: &str,
    each: impl Fn(&T) -> Value,
) -> Option<Value> {
    sub.try_get_many::<T>(name)
        .ok()
        .flatten()
        .map(|values| Value::List(values.map(each).collect()))
}

/// `name=value` or `name:value`, repeated.
fn pairs(matches: &ArgMatches, id: &str, between: char) -> Result<Vec<(String, String)>, String> {
    let Some(values) = matches.get_many::<String>(id) else {
        return Ok(Vec::new());
    };
    values
        .map(|raw| {
            raw.split_once(between)
                .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
                .ok_or_else(|| {
                    format!(
                        "`{raw}` is not `name{between}value`\n\
                         help: write `--{id} name{between}value`"
                    )
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command;
    use zod::Entry;

    #[derive(Entry)]
    struct Signup {
        #[text(min = 3, max = 16, slug)]
        handle: String,
        #[int(min = 13, max = 130)]
        age: i64,
        subscribed: bool,
        #[text(min = 2)]
        #[list(min = 1)]
        tags: Vec<String>,
        #[text(min = 4)]
        note: Option<String>,
    }

    fn routes() -> Vec<Described> {
        vec![
            Described {
                path: "/signup",
                verb: Verb::Post,
                takes: Some(Signup::describe()),
                gives: None,
            },
            Described {
                path: "/users/{id}",
                verb: Verb::Get,
                takes: None,
                gives: None,
            },
        ]
    }

    fn call(argv: &[&str]) -> Call {
        let routes = routes();
        let matches = command("blog", &routes)
            .try_get_matches_from(argv)
            .expect("parses");
        plan(&routes, &matches).expect("plans")
    }

    #[test]
    fn a_path_parameter_goes_into_the_url() {
        let call = call(&["blog", "users-id-get", "--id", "7"]);
        assert_eq!(call.verb, Verb::Get);
        assert_eq!(call.url, "http://127.0.0.1:3000/users/7");
        assert_eq!(call.body, None);
    }

    #[test]
    fn the_base_is_a_flag_and_a_trailing_slash_does_not_double() {
        let call = call(&[
            "blog",
            "--base",
            "https://example.com/",
            "users-id-get",
            "--id",
            "7",
        ]);
        assert_eq!(call.url, "https://example.com/users/7");
    }

    #[test]
    fn the_flags_become_the_body_in_the_order_the_struct_declares_them() {
        let call = call(&[
            "blog",
            "signup-post",
            "--handle",
            "ada-lovelace",
            "--age",
            "36",
            "--tags",
            "one",
            "--tags",
            "two",
            "--subscribed",
        ]);
        assert_eq!(
            call.body.as_deref(),
            Some(r#"{"handle":"ada-lovelace","age":36,"subscribed":true,"tags":["one","two"]}"#)
        );
    }

    #[test]
    fn a_flag_nobody_gave_is_a_field_nobody_sends() {
        let call = call(&[
            "blog",
            "signup-post",
            "--handle",
            "ada-lovelace",
            "--age",
            "36",
            "--tags",
            "x",
        ]);
        let body = call.body.unwrap();
        assert!(
            !body.contains("note"),
            "an Option left out is left out: {body}"
        );
        // A required boolean is a flag, so absent means false and the
        // field is still sent. It has no third value to keep.
        assert!(body.contains(r#""subscribed":false"#), "{body}");
    }

    #[test]
    fn a_body_that_is_sent_says_what_it_is() {
        let call = call(&[
            "blog",
            "signup-post",
            "--handle",
            "ada-lovelace",
            "--age",
            "36",
            "--tags",
            "x",
        ]);
        assert_eq!(
            call.headers,
            vec![("content-type".to_string(), "application/json".to_string())]
        );
    }

    #[test]
    fn a_content_type_the_caller_set_is_not_overridden() {
        let call = call(&[
            "blog",
            "-H",
            "content-type: application/vnd.api+json",
            "signup-post",
            "--handle",
            "ada-lovelace",
            "--age",
            "36",
            "--tags",
            "x",
        ]);
        assert_eq!(call.headers.len(), 1);
        assert_eq!(call.headers[0].1, "application/vnd.api+json");
    }

    #[test]
    fn whole_json_replaces_the_required_flags() {
        let call = call(&[
            "blog",
            "signup-post",
            "--body",
            r#"{"handle":"someone-else"}"#,
        ]);
        assert_eq!(call.body.as_deref(), Some(r#"{"handle":"someone-else"}"#));
    }

    #[test]
    fn whole_json_and_a_field_flag_do_not_mix() {
        let routes = routes();
        let error = command("blog", &routes)
            .try_get_matches_from([
                "blog",
                "signup-post",
                "--handle",
                "ada-lovelace",
                "--body",
                r#"{"handle":"someone-else"}"#,
            ])
            .expect_err("one body, not two");
        assert!(error.to_string().contains("--body"), "{error}");
    }

    #[test]
    fn a_path_value_is_encoded_so_it_stays_one_segment() {
        let call = call(&["blog", "users-id-get", "--id", "7/edit?x=1 y"]);
        assert_eq!(call.url, "http://127.0.0.1:3000/users/7%2Fedit%3Fx%3D1%20y");
    }

    #[test]
    fn a_catch_all_value_keeps_its_slashes() {
        let routes = vec![Described {
            path: "/files/{*rest}",
            verb: Verb::Get,
            takes: None,
            gives: None,
        }];
        let matches = command("blog", &routes)
            .try_get_matches_from(["blog", "files-rest-get", "--rest", "a b/c"])
            .expect("parses");
        let call = plan(&routes, &matches).expect("plans");
        assert_eq!(call.url, "http://127.0.0.1:3000/files/a%20b/c");
    }

    #[test]
    fn a_list_of_numbers_is_sent_as_numbers() {
        #[derive(Entry)]
        struct Pick {
            ids: Vec<i64>,
        }
        let routes = vec![Described {
            path: "/pick",
            verb: Verb::Post,
            takes: Some(Pick::describe()),
            gives: None,
        }];
        let matches = command("blog", &routes)
            .try_get_matches_from(["blog", "pick-post", "--ids", "1", "--ids", "2"])
            .expect("parses");
        let call = plan(&routes, &matches).expect("plans");
        assert_eq!(call.body.as_deref(), Some(r#"{"ids":[1,2]}"#));
    }

    #[test]
    fn a_field_named_like_a_path_parameter_or_a_flag_goes_through_body() {
        #[derive(Entry)]
        struct Comment {
            id: i64,
            body: String,
            title: String,
        }
        let routes = vec![Described {
            path: "/posts/{id}",
            verb: Verb::Put,
            takes: Some(Comment::describe()),
            gives: None,
        }];
        let tree = command("blog", &routes);
        // No duplicate id: clap's debug assertions accept the tree.
        tree.clone().debug_assert();
        let matches = tree
            .try_get_matches_from(["blog", "posts-id-put", "--id", "7", "--title", "Hi"])
            .expect("parses");
        let call = plan(&routes, &matches).expect("plans");
        assert_eq!(call.url, "http://127.0.0.1:3000/posts/7");
        assert_eq!(call.body.as_deref(), Some(r#"{"title":"Hi"}"#));
    }

    #[test]
    fn the_default_base_is_the_callers() {
        let routes = routes();
        let matches = crate::command_at("blog", &routes, "http://127.0.0.1:8080")
            .try_get_matches_from(["blog", "users-id-get", "--id", "7"])
            .expect("parses");
        let call = plan(&routes, &matches).expect("plans");
        assert_eq!(call.url, "http://127.0.0.1:8080/users/7");
    }

    #[test]
    fn a_query_parameter_is_escaped_so_it_stays_one_parameter() {
        let call = call(&["blog", "-q", "q=a&b c", "users-id-get", "--id", "7"]);
        assert_eq!(call.url, "http://127.0.0.1:3000/users/7?q=a%26b%20c");
    }

    #[test]
    fn a_pair_that_is_not_a_pair_is_refused_with_the_text_that_was_given() {
        let routes = routes();
        let matches = command("blog", &routes)
            .try_get_matches_from(["blog", "-H", "nonsense", "users-id-get", "--id", "7"])
            .expect("clap takes any string");
        let error = plan(&routes, &matches).expect_err("but this is not a header");
        assert!(error.contains("nonsense"), "{error}");
        assert!(error.contains("\nhelp: "), "{error}");
    }

    #[test]
    fn an_operation_with_no_body_sends_none() {
        let call = call(&["blog", "users-id-get", "--id", "7"]);
        assert!(call.body.is_none());
        assert!(call.headers.is_empty());
    }
}
