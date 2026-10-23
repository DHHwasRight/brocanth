# The surface

One table. A document, a client and the handlers all read it, so none of
them can disagree with the others.

Written 23 October 2026.

## 1. The signature is the declaration

```rust
pub fn handle_post(ctx: &mut Ctx, body: Signup) -> Result<Json<Created>, Error>
//                                      ^ takes              ^ gives
```

Four facts, none of them written twice:

| | where it comes from |
|---|---|
| the path | the file: `src/pages/signup.rs` is `/signup` |
| the verb | the function's name |
| the request body | the second parameter |
| the answer | the `T` of a `Json<T>` return |

`pages!` already parsed every page with `syn` to find the verbs. It now
reads the rest of the signature as well, and emits two things: a route
registered through `page_with_body`, which reads and checks the body
before the handler runs, and a row in `pages::described()`.

A page that writes `-> Result<impl IntoResponse, Error>` keeps working and
contributes no schema. The wrapper is the opt-in, which is why `takes` and
`gives` are `Option`.

## 2. One table, three readers

```rust
pub struct Described {
    pub path: &'static str,     // "/users/{id}"
    pub verb: Verb,
    pub takes: Option<zod::Node>,
    pub gives: Option<zod::Node>,
}
```

- **The handler** gets a `Signup` that has already passed, or never runs.
- **The document** gets `Signup` and `Created` as components, with the
  rules as JSON Schema keywords.
- **The client** gets `--handle` and `--age`, with the field's own sentence
  as the help and the schema's bounds as the parser's range.

Nothing here is written by hand, so nothing here can be out of date. That
is the whole claim, and it is checked by the compiler rather than by a
review: change `age` to a `u8` and the document, the flag and the handler
move together or the build stops.

## 3. The document

```
$ curl localhost:3000/openapi.json
```

```json
"Signup": {
  "type": "object",
  "properties": {
    "handle": {
      "type": "string",
      "title": "Handle",
      "description": "Pick 3 to 16 lowercase letters, digits or hyphens",
      "minLength": 3, "maxLength": 16,
      "pattern": "^[a-z0-9]+(-[a-z0-9]+)*$"
    }
  },
  "required": ["handle", "age", "subscribed"]
}
```

The description is the sentence the field already carries for its error
message. One sentence, written once, read before the request in the
document and after it in the 422.

`src/plugins/openapi/mod.rs` used to say:

> What a page accepts and returns is not derivable from a `Fn(&mut Ctx)`,
> so bodies are added by hand.

It does not say that any more when `input` is chosen. `.describe(..)` is
still there for a summary, a second status or an example — the things a
signature genuinely cannot say.

## 4. The client

```
$ my-blog api --help
  signup-post   POST /signup
  users-id-get  GET /users/{id}

$ my-blog api signup-post --handle ada-lovelace --age 36
{"id":"0191a3f0-0000-7000-8000-000000000000","handle":"ada-lovelace"}
```

Built from the table at run time, not generated: there is no file to
regenerate and nothing to keep in step. A path parameter is a required
flag, so a URL with `{id}` still in it never leaves the machine. An
integer's range comes from the schema, so `--age 200` is refused before a
socket is opened.

**Nothing in the client re-checks a rule.** The server is the authority,
and a client that validated as well would be a second opinion that can
disagree with it. What the rules do here is write the help: a person reads
"3 to 16 characters, a slug" before they send, instead of after.

Two conventions worth stating, because they are choices:

- A **required boolean is a flag**: `--subscribed` means yes, absent means
  no. It has no third value, and making somebody write `--subscribed false`
  for the ordinary case would be faithful and unusable. An `Option<bool>`
  keeps all three.
- A field no flag can carry — a nested object, a list of them — gets no
  flag, and `--body '<json>'` is how it is sent. Pretending a command line
  can carry a tree is how a client ends up with `--address.street`.

### Why this is not the client generator the openapi crate argues against

`brocanth-openapi`'s header says:

> `openapi-generator` emits clients for fifty languages [...] Writing a
> fifty-first would mean competing with tools that have years of
> per-language corner cases in them, and winning nothing.

That argument is right about what it is about, and this is not that. It
emits no files, it is one language — the one the server is already written
in — and the binary already holds the table, so answering `--help` from it
costs nothing. The honest cost: a client built from the table ships with
the server. A client for a server you do not build is still what
`openapi-generator` is for.

## 5. What this needed on the way

| | |
|---|---|
| `zod::Node` | a schema can describe itself, with the struct's own name |
| `zod::Give` + `zod::write` | a struct can be written back out as JSON, with no serde |
| `#[text(..)]` `#[int(..)]` | the rules as an annotation as well as a value |
| `Slot::CommandsMod` | a module can add a subcommand. Four of fifteen slots are read now |
| `Dep::feature` | declared, merged across modules, and never written into the manifest. A module could ask for a feature and not get it |
| `page_with_body`, `Json<T>` | written into `src/pages/mod.rs` by the `input` module |

## 6. Not done

- **A type named in a signature must be reachable from the page module.**
  The table is emitted one level up, so `Signup` is said as
  `signup::Signup` and the struct has to be `pub`. A `use` that is not
  `pub use` will not resolve, and the error says so.
- **No second status.** `Json<T>` is a 200. A 201 is `(Json(x), 201)` in
  the handler and `.describe(..)` in the document; the table carries one
  answer.
- **No request headers or query in the document.** The table has the path
  and the body. A query parameter is `--query k=v` on the client and
  nothing in the document yet.
- **`zod` is still not published**, so a project generated without
  `--brocanth-path` writes a git URL that does not resolve. See
  `docs/input.md` §7.
