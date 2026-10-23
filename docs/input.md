# The entrance

What a request is made of, how it becomes a struct, and what happens when
it cannot.

Written 23 October 2026, with `brocanth-input`.

## 1. What was missing, exactly

A generated project's `Ctx` carried four things: a verb, a path, the route
parameters, and a partial-navigation depth. It did not carry the body.

```rust
pub struct Ctx {
    pub verb: Verb,
    pub path: String,
    params: Vec<(String, String)>,
    partial: u32,
}
```

`dispatch` had the whole `brocanth_server::Request` in hand -- query string,
headers, body -- and took two fields off it. So `handle_post` could not read
a form. Not "read it by hand", which is what `docs/middleware.md` §4 said:
**it could not read one at all.**

That also stranded `brocanth-schema`. `Validation::check<T>(value: T)` takes
a `T` that already exists, and nothing in a generated project could make
one from a request. The typed witness `Valid<T>` was unreachable by
construction.

Two changes close it:

1. `Ctx` gains `query`, `body` and the headers, filled by `Ctx::sent` in
   `dispatch`. Every project gets this, because a context that cannot see
   the request is a hole whatever else was chosen.
2. The `input` module writes an `impl Ctx` that turns those three into a
   struct.

## 2. The two crates, and which answers what

| | `brocanth-schema` (`validation`) | `brocanth-input` (`input`) |
|---|---|---|
| question | is this value, which I hold, valid? | what arrived, and is it a `T`? |
| input | a `T` that already exists | bytes |
| gives | `Valid<T>`, a typed witness | a `T`, or every fault |
| reads the request | no | the content type, the query, the body |
| vocabulary | `brocanth_schema::{string, int, ..}` | `zod::{Text, Int, ..}` |

They do not overlap and neither needs the other. A project may take both:
`input` to get the struct, `Valid<T>` to carry the proof onward.

Two vocabularies is a real cost and it is worth naming. `brocanth-schema`
grew from "validate a struct and describe it for OpenAPI and the admin".
`zod` grew from "a rule that is a value, which the compiler can also run".
The second does something the first cannot, and §5 is that difference.

## 3. One struct, every source

`zod::Input` carries the data and says what the source is made of. That,
not the schema, decides whether text may become a number.

| Source | Shape | `"36"` becomes a number |
|---|---|---|
| a JSON body | typed | no |
| a form | flat | yes |
| a query string | flat | yes |
| the environment | flat | yes |

A JSON body that sends text for a number has a bug in the sender, and a
schema that hides it makes the bug permanent. A form has no types to lose.
A field that must accept both says `.loose()`.

So one struct reads a form POST and a JSON POST with nothing switched:

```rust
#[derive(zod::Entry)]
struct Signup {
    #[check(zod::Text::new().min(3).max(16).slug())]
    #[label("Handle")]
    #[says("Pick 3 to 16 lowercase letters, digits or hyphens")]
    handle: String,

    #[check(zod::Int::new().from(13).to(130))]
    age: i64,

    subscribed: bool,
}

pub fn handle_post(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let signup: Signup = ctx.body_as()?;
    Ok(Response::empty(303).with_header("location", "/"))
}
```

Three form rules, each of them a bug somebody has shipped: a field with no
value is nothing rather than empty text; a name that repeats becomes a list,
which is what a group of checkboxes sends; an unticked checkbox sends
nothing at all, and a boolean reads that as false.

## 4. Three answers, three statuses

`ctx.read::<T>()` keeps them apart, because collapsing them loses the
status:

| | | status |
|---|---|---|
| `Sent::Good(T)` | it read, every rule passed | -- |
| `Sent::Faulty(Faults)` | it read, these rules did not | 422 |
| `Sent::Unreadable(..)` | nobody could read it | 400 or 415 |

A body that broke a rule is something the client can fix. A body nobody
could read is not, and no amount of correcting fields will help.

`ctx.body_as()` collapses all three into `Result<T, Error>` for a handler
that only wants `?`. An HTML page matches on `Sent::Faulty` instead and
draws the form again with `faults.by_name()` beside the inputs.

What `refused` builds depends on what the project is. An application with
templates gets one line per field, which a handler usually replaces. One
without gets `application/problem+json` by RFC 9457: a status, a title and
one entry per fault with its path, its code and the numbers it compared.

### A trap the generated code documents

```rust
let signup: Signup = Signup::parse(input)?;   // 500. Wrong.
```

`Error` has a blanket `impl<E: std::error::Error> From<E>` that answers
500, and `zod::Faults` is a `std::error::Error`. So `?` on a `Faults`
compiles and gives the wrong status. `body_as` is the call that gets it
right, and the generated comment and `AGENTS.md` both say so.

## 5. What the compiler checks

`brocanth-schema`'s own header says it plainly:

> A request body is bytes that did not exist when you compiled, so no type
> system anywhere can check them in advance.

True of a request. Not true of a literal, and that is the gap `zod` fills.
The rules are values, so the same `const` serves the field and the
compiler:

```rust
const HANDLE: zod::Text = zod::Text::new().min(3).max(16).slug();

zod::accepts!(HANDLE, "ada-lovelace");   // the compiler runs the rule
```

Change it to `"A"` and the build stops with `evaluation panicked: too
short` on that line. A seed, a fixture, a redirect target, a default
configuration value: every literal a project writes against its own rules
can be checked before it runs.

`validator`, `garde` and `nutype` cannot do this, and neither can
`brocanth-schema`, for the same reason in all four: the rule is an
attribute or a macro argument, and an attribute is not a value, so no
function takes one.

## 6. What this changed in the generator

| | |
|---|---|
| `Ctx` | gains `query`, `body`, `headers`, and `header` `headers` `content_type` `body_text` `sent` |
| `Error` | gains `headers` and `with_header`, so a failure can set a content type |
| `dispatch` | fills the context from the request it already had |
| `Slot::PagesItems` | read for the first time. Five of the fifteen slots are still unread |
| `brocanth_server::Response` | gains `header` and `headers`, which `Request` always had |
| local mode | `--brocanth-path` now also resolves a sibling checkout for a git dependency, which is what lets `the_entrance_compiles` build against work that is not pushed |

`the_entrance_compiles` is the test that matters: it generates a project
with `input`, writes a page that reads a form into a struct, and runs
`cargo build` over it. The `impl Ctx` the module writes has to agree with
the `Ctx` the generator writes above it, and nothing else checks that.

## 7. Not done

- **`zod` is not published.** The generated `Cargo.toml` writes
  `zod = { git = "https://github.com/DHHwasRight/zod.git", branch = "main" }`,
  the same shape `rsx` uses, and that URL does not exist yet. A project
  generated with `--brocanth-path` resolves it from the sibling checkout
  and builds; one generated without it will not until the repository is
  pushed. This is the one manual step.
- **No multipart.** An upload is `multipart/form-data` and `Entrance`
  answers 415 for it. `brocanth-storage` is where the other half of that
  lives.
- **No streaming body.** `Ctx::body` is a `Vec<u8>`, so the entrance reads
  what the server already buffered, up to `MAX_BODY`.
- **One copy per request.** The query, the body and the headers are copied
  out of the server's request, which lives for the connection while the
  context lives for the handler. Borrowing would put a lifetime on `Ctx`,
  and `Ctx` is in `Handler`, `MiddlewareStack` and every page signature.
- **`Valid<T>` and `Entry` are not bridged.** Both work; a struct that
  wants both writes both derives.
