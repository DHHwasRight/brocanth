# brocanth-input

The entrance: a request body becomes the struct a handler asked for, or
an answer that says why not. `Entrance` reads three parts of a request,
the content type, the query string and the body. It gives a `zod::Input`,
and a struct that derives `zod::Entry` reads itself from it. The same
struct reads a urlencoded form, a JSON body, the text fields of a
`multipart/form-data` upload, and a query string. Every broken rule comes
back at once, never only the first. Use it in a handler that reads a body
or a query string. It does not validate a value that you already hold:
that is `brocanth-schema` and its `Valid<T>`. You can use both.

Three outcomes, three statuses:

| Outcome | Type | Status |
|---|---|---|
| The body reads, and every rule passes | your `T` | the handler decides |
| The body reads, and some rules fail | `zod::Faults` | 422 |
| Nobody can read the body | `Unreadable` | 400, 413 or 415 (`Unreadable::status`) |

## Getting started

```toml
[dependencies]
brocanth-input = { git = "https://github.com/DHHwasRight/brocanth.git" }
zod = { git = "https://github.com/DHHwasRight/zod.git", branch = "main" }
# Optional feature:
#   scaffold  - the `brocanth create` module; only the CLI turns it on
```

Inside this workspace: `brocanth-input.workspace = true`. The crate
re-exports `zod` as `brocanth_input::zod`, but `#[derive(zod::Entry)]`
needs `zod` as a direct dependency.

### As a module

```text
brocanth create my-app --with input
brocanth add input
```

The module adds `brocanth-input` and `zod` to `Cargo.toml`. It writes into
the generated pages, and registers no plugin:

- `ctx.read::<T>()`, which gives a `Sent<T>`: `Good`, `Faulty` or
  `Unreadable`.
- `ctx.body_as::<T>()` and `ctx.query_as::<T>()`, which give a
  `Result<T, Error>` with the right status.
- `page_with_body`, so a page whose handler takes a second parameter
  (`fn handle_post(ctx: &mut Ctx, body: Signup)`) gets its body read and
  checked before it runs.
- `Json<T>`, a response wrapper that publishes `T` as the response schema.
- `refused`: a 422 with `application/problem+json`, or one line per field
  when the project also has `rsx`.

These are generated code in the project, not items of this crate. The
module declares no settings: no flag, no environment variable, no config
key.

## Examples

### A form and a JSON body read into one struct

```rust
use brocanth_input::Entrance;
use zod::{Entry, Int, Text};

#[derive(Entry, Debug)]
struct Signup {
    #[check(Text::new().min(3).max(16).slug())]
    handle: String,
    #[check(Int::new().from(13).to(130))]
    age: i64,
}

let form = Entrance::new(
    Some("application/x-www-form-urlencoded"),
    "",
    b"handle=ada-lovelace&age=36",
);
assert_eq!(Signup::parse(form.input().unwrap()).unwrap().age, 36);

let json = Entrance::new(
    Some("application/json"),
    "",
    br#"{"handle": "ada-lovelace", "age": 36}"#,
);
assert_eq!(Signup::parse(json.input().unwrap()).unwrap().age, 36);

// A form has no types to lose, so `age=36` is a number there.
// A JSON body that sends text for a number has a bug, and this says so.
let wrong = Entrance::new(
    Some("application/json"),
    "",
    br#"{"handle": "ada-lovelace", "age": "36"}"#,
);
let faults = Signup::parse(wrong.input().unwrap()).unwrap_err();
assert_eq!(faults.all()[0].flaw.code, zod::Code::WrongType);

// The query string is a form by another name.
let asked = Entrance::new(None, "?handle=ada-lovelace&age=36", b"");
assert!(Signup::parse(asked.asked()).is_ok());
```

### Answer a refusal with the right status

`Answer` turns `Faults` into a 422 `application/problem+json` response
(RFC 9457). `unreadable` turns an `Unreadable` into a plain-text 400, 413
or 415.

```rust
use brocanth_input::{unreadable, Answer, Entrance, Unreadable};
use zod::{Entry, Int, Refusal, Text};

#[derive(Entry, Debug)]
struct Signup {
    #[check(Text::new().min(3).max(16).slug())]
    handle: String,
    #[check(Int::new().from(13).to(130))]
    age: i64,
}

fn answer(content_type: Option<&str>, body: &[u8]) -> brocanth_server::Response {
    let at = Entrance::new(content_type, "", body);
    let input = match at.input() {
        Ok(input) => input,
        Err(why) => return unreadable(&why),
    };
    match Signup::parse(input) {
        Ok(_signup) => brocanth_server::Response::text(201, "created"),
        Err(faults) => Answer.refuse(&faults),
    }
}

let form = Some("application/x-www-form-urlencoded");
assert_eq!(answer(form, b"handle=ada-lovelace&age=36").status, 201);

// Both fields are wrong, and both faults are in the one answer.
let refused = answer(form, b"handle=A&age=200");
assert_eq!(refused.status, 422);
assert_eq!(refused.header("content-type"), Some("application/problem+json"));

// No content type: refused rather than guessed.
assert_eq!(answer(None, b"handle=ada").status, 415);
assert_eq!(Unreadable::Unsaid.status(), 415);
```

### Draw the form again, with each field's messages

`rerender` calls the same template function that drew the form the first
time, with the errors this time, and answers 422.

```rust
use brocanth_input::{rerender, Entrance, FieldErrors};
use zod::{Entry, Text};

#[derive(Entry, Debug)]
struct Item {
    #[check(Text::new().min(3))]
    name: String,
}

fn form(errors: &FieldErrors) -> String {
    format!(
        r#"<form method="post"><input name="name" aria-invalid="{}">{}</form>"#,
        errors.has("name"),
        errors.render("name"),
    )
}

// The first render has no errors.
assert!(!form(&FieldErrors::none()).contains("field-error"));

let at = Entrance::new(Some("application/x-www-form-urlencoded"), "", b"name=ab");
let faults = Item::parse(at.input().unwrap()).unwrap_err();
let sent = rerender(&faults, form);
assert_eq!(sent.status, 422);
assert_eq!(sent.header("content-type"), Some("text/html; charset=utf-8"));
```

### An upload: the text fields to the struct, the files beside it

```rust
use brocanth_input::{Entrance, Limits};
use zod::{Entry, Text};

#[derive(Entry, Debug)]
struct Post {
    #[check(Text::new().min(1))]
    title: String,
}

let body = b"--x\r\n\
    Content-Disposition: form-data; name=\"title\"\r\n\r\n\
    A cat\r\n\
    --x\r\n\
    Content-Disposition: form-data; name=\"photo\"; filename=\"cat.png\"\r\n\
    Content-Type: image/png\r\n\r\n\
    \x89PNG\r\n\
    --x--\r\n";

let at = Entrance::new(Some("multipart/form-data; boundary=x"), "", body);
let post = Post::parse(at.input().unwrap()).unwrap();
assert_eq!(post.title, "A cat");

let sent = at.multipart().unwrap();
let photo = sent.file("photo").unwrap();
assert_eq!(photo.filename(), Some("cat.png"));
assert_eq!(photo.declared_type(), Some("image/png")); // a claim, not a fact
assert_eq!(photo.bytes(), b"\x89PNG");

// A body over a cap is a 413.
let tight = Limits { total_bytes: 16, ..Limits::DEFAULT };
let why = at.with_limits(tight).multipart().unwrap_err();
assert_eq!(why.status(), 413);
```

This crate does not sniff the bytes. `brocanth-storage` reads the real
media type when it stores the file.

## Testing

```text
cargo test -p brocanth-input
```

No database and no environment variable. The tests are unit tests and
doctests.

`Entrance` takes three borrowed parts and no framework type, so a unit
test builds one directly, as the examples do. To test a page that reads a
body, send the request through `brocanth_test::TestClient`:
`client.post("/signup", &[("handle", "ada-lovelace"), ("age", "36")])` or
`TestRequest::post(path).form(..)`, then assert on the status (201, 422,
415) and on the re-rendered form with the `Html` reader. See
`crates/brocanth-test/README.md`.

## More

- `docs/input.md`: what was missing, the three carriers, the statuses, and
  how `zod` differs from `brocanth-schema`.
- `docs/modules.md`: how `input` reaches a generated project.
- The `described` module: the route table (`Described`) that
  `brocanth-openapi` and `brocanth-client` read.
