//! A command line that is a client for the application that generated it.
//!
//! The table `brocanth_pages::pages!("src/pages", describe)` writes already
//! says every path, every verb, every path parameter and the shape of every
//! body. That is what a client is. So this builds one out of it rather than
//! out of a document somebody generated out of it:
//!
//! ```text
//! $ blog signup-post --handle ada-lovelace --age 36
//! {"id":"0191a3f0-0000-7000-8000-000000000000","handle":"ada-lovelace"}
//! ```
//!
//! # Why not a document
//!
//! `brocanth-openapi`'s header argues against client generators, and the
//! argument is right about what it is about:
//!
//! > `openapi-generator` emits clients for fifty languages [...] Writing a
//! > fifty-first would mean competing with tools that have years of
//! > per-language corner cases in them.
//!
//! This is not that. It is not a code generator, it emits no files, and it
//! is one language -- the one the server is already written in. A binary
//! that already holds the table can answer `--help` from it, and the
//! alternative is asking a person to install a toolchain to regenerate a
//! client every time a handler changes.
//!
//! The honest cost, stated: a client built from the table is in the same
//! binary as the table, so it ships with the server. A client for a server
//! you do not build is what `openapi-generator` is for, and that is still
//! true.
//!
//! # What a person gets
//!
//! One subcommand per operation, named `path-verb`. One `--flag` per field
//! of the request body, required when the schema requires it, with the
//! field's own sentence as its help. Path parameters are flags too. A
//! `--body` that takes whole JSON, for a body no flag can express.
//!
//! Nothing here re-checks a rule. The server is the authority and a client
//! that validated too would be a second opinion that can disagree with it.
//! What the rules do here is write the help, so a person reads "3 to 16
//! characters, a slug" before they send instead of after.

#![deny(missing_docs)]

use std::collections::BTreeMap;

use brocanth::routing::Verb;
use brocanth_input::Described;
use clap::{Arg, ArgAction, Command};
use zod::node::{Field, Node};

pub mod help;
mod plan;

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "reqwest-client")]
mod reqwest_wire;

pub use plan::{plan, Call};

#[cfg(feature = "reqwest-client")]
pub use reqwest_wire::Reqwest;

/// How a request is actually sent.
///
/// The contract, so an application holding its own HTTP client implements
/// this and the shipped one appears nowhere in its lock file.
pub trait Wire {
    /// Send it, and give back what came back.
    ///
    /// # Errors
    ///
    /// Anything that stopped the request reaching an answer. A 500 is an
    /// answer and is not an error here.
    fn send(&self, call: &Call) -> Result<Answer, String>;
}

/// What came back.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Answer {
    /// The HTTP status.
    pub status: u16,
    /// The body, as text.
    pub body: String,
    /// What the body is written in, when the server said.
    pub content_type: Option<String>,
}

impl Answer {
    /// Whether the server accepted the request.
    pub const fn ok(&self) -> bool {
        self.status < 400
    }
}

/// The command tree for a surface.
///
/// `name` is the program's own name, as `--help` should print it.
pub fn command(name: &'static str, routes: &[Described]) -> Command {
    let mut root = Command::new(name)
        .about("Talk to this application over HTTP")
        .arg(
            Arg::new("base")
                .long("base")
                .global(true)
                .value_name("URL")
                .help("Where the server is")
                .default_value("http://127.0.0.1:3000"),
        )
        .arg(
            Arg::new("header")
                .long("header")
                .short('H')
                .global(true)
                .value_name("NAME:VALUE")
                .action(ArgAction::Append)
                .help("A header to send. Repeat for more than one"),
        )
        .arg(
            Arg::new("query")
                .long("query")
                .short('q')
                .global(true)
                .value_name("NAME=VALUE")
                .action(ArgAction::Append)
                .help("A query parameter. Repeat for more than one"),
        )
        .subcommand_required(true)
        .arg_required_else_help(true);

    for route in routes {
        root = root.subcommand(operation(route));
    }
    root
}

/// One operation, as a subcommand.
fn operation(route: &Described) -> Command {
    let mut command =
        Command::new(route.command()).about(format!("{} {}", route.verb.as_str(), route.path));

    for name in route.parameters() {
        command = command.arg(
            Arg::new(flag(name))
                .long(flag(name))
                .value_name(name.to_ascii_uppercase())
                .required(true)
                .help(format!("The `{name}` in the path")),
        );
    }

    if let Some(object) = route.takes.as_ref().and_then(Node::object) {
        for field in &object.fields {
            if let Some(arg) = argument(field) {
                command = command.arg(arg);
            }
        }
        command = command.arg(
            Arg::new("body")
                .long("body")
                .value_name("JSON")
                .help("The whole body as JSON, instead of the flags above"),
        );
    }
    command
}

/// One field of the body, as a flag.
///
/// A field no flag can express -- a nested object, a list of them -- gets
/// none, and `--body` is how it is sent. Pretending a command line can
/// carry a tree is how a client ends up with `--address.street`.
fn argument(field: &Field) -> Option<Arg> {
    let name = flag(field.name);
    let mut arg = Arg::new(name.clone())
        .long(name)
        .value_name(field.name.to_ascii_uppercase())
        .required(field.required)
        .help(help::sentence(field));

    match field.node.bare() {
        Node::Bool if field.required => {
            // A required boolean has two values and no third, so absent
            // means false -- which is what every command line does and
            // what the schema can carry without losing anything. Making a
            // person write `--subscribed false` to mean the ordinary case
            // would be faithful and unusable.
            arg = arg.required(false).action(ArgAction::SetTrue);
        }
        Node::Bool => {
            // An `Option<bool>` has a third value, so it keeps one: given
            // and true, given and false, or not given at all.
            arg = arg
                .num_args(0..=1)
                .default_missing_value("true")
                .value_parser(clap::value_parser!(bool));
        }
        Node::Int(facts) => {
            arg = arg.value_parser(clap::value_parser!(i64).range(
                facts.min.max(i128::from(i64::MIN)) as i64
                    ..=facts.max.min(i128::from(i64::MAX)) as i64,
            ));
        }
        Node::Real(_) => arg = arg.value_parser(clap::value_parser!(f64)),
        Node::List { item, .. } => {
            if matches!(item.bare(), Node::Object(_)) {
                return None;
            }
            arg = arg.action(ArgAction::Append);
        }
        Node::Object(_) => return None,
        _ => {}
    }
    Some(arg)
}

/// A field name as a long flag: hyphens, because that is what a command
/// line uses and `--phone_number` is not one.
fn flag(name: &str) -> String {
    name.replace('_', "-")
}

/// Build the command tree, parse these arguments, send the request.
///
/// # Errors
///
/// A message ready to print. Parsing failures are clap's own, already
/// written for a person.
pub fn run<W: Wire>(
    name: &'static str,
    routes: &[Described],
    wire: &W,
    argv: impl IntoIterator<Item = std::ffi::OsString>,
) -> Result<Answer, String> {
    let matches = command(name, routes)
        .try_get_matches_from(argv)
        .map_err(|e| e.to_string())?;
    let call = plan(routes, &matches)?;
    wire.send(&call)
}

/// Every operation, as one line each: what `--help` would say, without
/// going through clap.
pub fn listing(routes: &[Described]) -> Vec<(String, String)> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for route in routes {
        out.insert(
            route.command(),
            format!("{} {}", route.verb.as_str(), route.path),
        );
    }
    out.into_iter().collect()
}

/// The verb, for a sender that wants a method name.
pub const fn method(verb: Verb) -> &'static str {
    verb.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zod::Entry;

    #[derive(Entry)]
    struct Signup {
        #[text(min = 3, max = 16, slug)]
        #[label("Handle")]
        #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
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

    #[test]
    fn one_subcommand_per_operation_named_for_the_path_and_the_verb() {
        let names = listing(&routes());
        assert_eq!(names[0].0, "signup-post");
        assert_eq!(names[0].1, "POST /signup");
        assert_eq!(names[1].0, "users-id-get");
    }

    #[test]
    fn a_path_parameter_is_a_required_flag() {
        let tree = command("blog", &routes());
        let matches = tree
            .try_get_matches_from(["blog", "users-id-get", "--id", "7"])
            .expect("the flag is there");
        let (_, sub) = matches.subcommand().unwrap();
        assert_eq!(sub.get_one::<String>("id").map(String::as_str), Some("7"));
    }

    #[test]
    fn a_path_parameter_that_is_missing_is_refused_before_a_request_is_made() {
        let tree = command("blog", &routes());
        let error = tree
            .try_get_matches_from(["blog", "users-id-get"])
            .expect_err("a URL with {id} still in it is a 404 worth skipping");
        assert!(error.to_string().contains("--id"), "{error}");
    }

    #[test]
    fn a_required_field_is_a_required_flag_and_an_option_is_not() {
        let tree = command("blog", &routes());
        let error = tree
            .clone()
            .try_get_matches_from(["blog", "signup-post", "--age", "36", "--tags", "x"])
            .expect_err("handle is required");
        assert!(error.to_string().contains("--handle"), "{error}");

        tree.try_get_matches_from([
            "blog",
            "signup-post",
            "--handle",
            "ada-lovelace",
            "--age",
            "36",
            "--tags",
            "one",
        ])
        .expect("note is an Option, so it is not required");
    }

    #[test]
    fn an_integer_outside_the_schemas_range_is_refused_by_the_flag() {
        let tree = command("blog", &routes());
        let error = tree
            .try_get_matches_from([
                "blog",
                "signup-post",
                "--handle",
                "ada-lovelace",
                "--age",
                "200",
                "--tags",
                "one",
            ])
            .expect_err("130 is the maximum the schema carries");
        assert!(error.to_string().contains("200"), "{error}");
    }

    #[test]
    fn a_required_boolean_is_a_flag_and_absent_means_no() {
        let tree = command("blog", &routes());
        let parse = |extra: &[&str]| {
            let mut argv = vec![
                "blog",
                "signup-post",
                "--handle",
                "ada-lovelace",
                "--age",
                "36",
                "--tags",
                "one",
            ];
            argv.extend_from_slice(extra);
            let matches = tree.clone().try_get_matches_from(argv).expect("parses");
            let (_, sub) = matches.subcommand().unwrap();
            sub.get_one::<bool>("subscribed").copied()
        };

        assert_eq!(parse(&["--subscribed"]), Some(true));
        assert_eq!(
            parse(&[]),
            Some(false),
            "a required boolean has no third value"
        );
    }

    #[test]
    fn an_optional_boolean_keeps_its_third_value() {
        #[derive(Entry)]
        struct Maybe {
            flag: Option<bool>,
        }
        let routes = vec![Described {
            path: "/m",
            verb: Verb::Post,
            takes: Some(Maybe::describe()),
            gives: None,
        }];
        let tree = command("blog", &routes);
        let parse = |extra: &[&str]| {
            let mut argv = vec!["blog", "m-post"];
            argv.extend_from_slice(extra);
            let matches = tree.clone().try_get_matches_from(argv).expect("parses");
            let (_, sub) = matches.subcommand().unwrap();
            sub.get_one::<bool>("flag").copied()
        };

        assert_eq!(parse(&["--flag"]), Some(true));
        assert_eq!(parse(&["--flag", "false"]), Some(false));
        assert_eq!(parse(&[]), None, "given, given as false, or not given");
    }

    #[test]
    fn a_list_field_repeats() {
        let tree = command("blog", &routes());
        let matches = tree
            .try_get_matches_from([
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
            ])
            .expect("parses");
        let (_, sub) = matches.subcommand().unwrap();
        let tags: Vec<&String> = sub.get_many::<String>("tags").unwrap().collect();
        assert_eq!(tags, vec!["one", "two"]);
    }

    #[test]
    fn a_flag_is_hyphenated_whatever_the_field_is_called() {
        #[derive(Entry)]
        struct Renamed {
            #[name("phone_number")]
            phone: Option<String>,
        }
        let routes = vec![Described {
            path: "/p",
            verb: Verb::Post,
            takes: Some(Renamed::describe()),
            gives: None,
        }];
        let tree = command("blog", &routes);
        tree.try_get_matches_from(["blog", "p-post", "--phone-number", "555"])
            .expect("the flag is hyphenated");
    }

    #[test]
    fn the_help_for_a_field_is_the_sentence_the_schema_carries() {
        let tree = command("blog", &routes());
        let rendered = tree
            .find_subcommand("signup-post")
            .expect("the operation")
            .clone()
            .render_help()
            .to_string();
        assert!(
            rendered.contains("Pick 3 to 16 lowercase letters"),
            "{rendered}"
        );
    }

    #[test]
    fn a_field_no_flag_can_express_is_left_to_the_body_flag() {
        #[derive(Entry)]
        struct Address {
            #[text(filled)]
            street: String,
        }
        #[derive(Entry)]
        struct Order {
            #[nested]
            ship_to: Address,
        }
        let routes = vec![Described {
            path: "/orders",
            verb: Verb::Post,
            takes: Some(Order::describe()),
            gives: None,
        }];
        let tree = command("shop", &routes);
        let operation = tree.find_subcommand("orders-post").expect("the operation");
        assert!(operation.get_arguments().all(|a| a.get_id() != "ship-to"));
        assert!(operation.get_arguments().any(|a| a.get_id() == "body"));
    }

    #[test]
    fn an_operation_with_no_body_has_no_body_flag() {
        let tree = command("blog", &routes());
        let operation = tree.find_subcommand("users-id-get").expect("the operation");
        assert!(operation.get_arguments().all(|a| a.get_id() != "body"));
    }
}
