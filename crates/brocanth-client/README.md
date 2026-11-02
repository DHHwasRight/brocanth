# brocanth-client

A command line that is a client for the application that holds it. It
reads the route table that `brocanth_pages::pages!("src/pages", describe)`
writes (`brocanth_input::Described`) and builds a `clap` command from it:
one subcommand per operation, named `path-verb`, one `--flag` per field of
the request body, one required flag per path parameter, and `--body` for
the whole body as JSON. The help text of a flag is the sentence the field's
schema carries. Use it to call your own application from a shell or a
script. It does not re-check a rule: the server is the authority, and a
422 from it carries every fault. It is not a code generator, and it is not
a client for a server that you do not build. Use an OpenAPI generator for
that.

```text
$ my-app api signup-post --handle ada-lovelace --age 36
{"id":"0191a3f0-0000-7000-8000-000000000000","handle":"ada-lovelace"}
```

## Getting started

```toml
[dependencies]
brocanth-client = { git = "https://github.com/DHHwasRight/brocanth.git" }
# Features:
#   reqwest-client  (default) - `Reqwest`, a blocking rustls sender
#   scaffold                  - the `brocanth create` module; only the CLI turns it on
```

Inside this workspace: `brocanth-client.workspace = true`.

`Wire` is the contract for sending. An application with its own HTTP
client sets `default-features = false`, implements `Wire`, and reqwest is
then not in its lock file.

### As a module

```text
brocanth create my-app --with input,commands,client
brocanth add client
```

`client` needs `input` (the route table) and `commands` (a place for a
subcommand). It writes `src/commands/api.rs`: an `api` command that calls
`brocanth_client::run` with `crate::pages::described()` and `Reqwest`. It
prints the body, and exits with 1 when the status is 400 or more. The
module declares no settings. The `api` command has its own global flags:
`--base URL` (default `http://127.0.0.1:3000`), `-H/--header NAME:VALUE`
and `-q/--query NAME=VALUE`.

## Examples

### Build the command tree from a route table

```rust
use brocanth::routing::Verb;
use brocanth_input::Described;
use zod::Entry;

#[derive(Entry)]
struct Signup {
    #[text(min = 3, max = 16, slug)]
    #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
    handle: String,
    #[int(min = 13, max = 130)]
    age: i64,
}

let routes = vec![
    Described { path: "/signup", verb: Verb::Post, takes: Some(Signup::describe()), gives: None },
    Described { path: "/users/{id}", verb: Verb::Get, takes: None, gives: None },
];

// One line per operation, without going through clap.
let listing = brocanth_client::listing(&routes);
assert_eq!(listing[0], ("signup-post".to_string(), "POST /signup".to_string()));
assert_eq!(listing[1].0, "users-id-get");

// The flags come from the schema: `--handle` is required, and `--age`
// refuses a value outside 13 to 130 before a request is made.
let tree = brocanth_client::command("my-app", &routes);
let error = tree
    .try_get_matches_from(["my-app", "signup-post", "--handle", "ada-lovelace", "--age", "200"])
    .unwrap_err();
assert!(error.to_string().contains("200"));
```

### Plan a request without sending it

`plan` turns parsed arguments into a `Call`: a verb, a full URL, headers
and a body. A test reads it without a server.

```rust
use brocanth::routing::Verb;
use brocanth_input::Described;

let routes = vec![Described { path: "/users/{id}", verb: Verb::Get, takes: None, gives: None }];
let matches = brocanth_client::command("my-app", &routes)
    .try_get_matches_from([
        "my-app", "--base", "https://example.test", "users-id-get", "--id", "7", "-q", "full=yes",
    ])
    .unwrap();

let call = brocanth_client::plan(&routes, &matches).unwrap();
assert_eq!(call.verb, Verb::Get);
assert_eq!(call.url, "https://example.test/users/7?full=yes");
assert_eq!(call.body, None);
```

### Send through your own `Wire`

```rust
use std::cell::RefCell;
use brocanth::routing::Verb;
use brocanth_client::{Answer, Call, Wire};
use brocanth_input::Described;

/// A sender that records the call and answers 404. A real one sends it.
struct Recorder(RefCell<Vec<Call>>);

impl Wire for Recorder {
    fn send(&self, call: &Call) -> Result<Answer, String> {
        self.0.borrow_mut().push(call.clone());
        Ok(Answer { status: 404, body: "not found".into(), content_type: None })
    }
}

let routes = vec![Described { path: "/users/{id}", verb: Verb::Get, takes: None, gives: None }];
let wire = Recorder(RefCell::new(Vec::new()));

let answer = brocanth_client::run("api", &routes, &wire, ["api", "users-id-get", "--id", "7"].map(Into::into))
    .expect("the request reached an answer");

// A 404 is an answer, not an error. The caller decides the exit code.
assert!(!answer.ok());
assert_eq!(wire.0.borrow()[0].url, "http://127.0.0.1:3000/users/7");
```

`brocanth_client::Reqwest::new()` is the shipped `Wire`, under the default
`reqwest-client` feature.

## Testing

```text
cargo test -p brocanth-client
```

No server, no database and no environment variable. The tests parse
arguments with `clap` and assert on the result.

An application tests its `api` command in two steps. Parse and plan with
`command` and `plan`, and assert on the `Call`, as the second example
does. Test the server side of each operation with
`brocanth_test::TestClient` (`crates/brocanth-test/README.md`). A fake
`Wire`, as in the third example, joins the two without a socket.

## More

- `docs/input.md`: the route table and the schemas the flags come from.
- `docs/modules.md`: the boundary crates (`input`, `openapi`, `client`)
  and how one table reaches each of them.
- `crates/brocanth-openapi`: the same table, published as a document.
