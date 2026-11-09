# brocanth-schema

`brocanth-schema` is the validation vocabulary of brocanth. You declare a
schema once, with `#[derive(Validate)]` on a struct or with a builder such
as `string().email()`. The compiler checks the shape of the schema. The
schema checks the data at run time and reports every issue with its path.
The same declaration gives a `SchemaNode`, which `brocanth-openapi` and
`brocanth-admin` read. Use this crate to check a value that you already
hold. To turn request bytes into a struct, use `brocanth-input` (see
`docs/input.md`). The two crates do not overlap, and a project can use
both.

## Getting started

```toml
[dependencies]
brocanth-schema = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

In this workspace, use `brocanth-schema.workspace = true`.

| Feature | Default | What it adds |
|---|---|---|
| `derive` | on | `#[derive(Validate)]` |
| `pattern` | on | `StringSchema::pattern`, with the `regex` crate |
| `plugin` | on | `ValidationPlugin`, `Validation` and `Valid<T>`, with `brocanth` |
| `serde` | off | `Serialize` and `Deserialize` on `SchemaNode`, `Issue` and `Issues` |
| `scaffold` | off | the `validation` module for the `brocanth` CLI |

With `default-features = false` the crate has no dependencies. You get the
vocabulary, `Issues` and `SchemaNode`.

### As a module

```sh
brocanth create my-app --with validation   # a new project
brocanth add validation                    # an existing project
```

The module writes `src/plugins/validation/mod.rs`, registers
`ValidationPlugin` and adds `brocanth-schema` to `Cargo.toml`. It declares
one setting (see `docs/module-flags.md`):

| Key | Flag | Variable | Default | Meaning |
|---|---|---|---|---|
| `report` | `--validation.report` | `$<APP>_VALIDATION__REPORT` | `json` | How a failed check is reported: `json` or `form`. Another value stops the application at startup. |

With the `config` module the key is in `[validation]` in
`config/<env>.toml`. With `config`, the flags are off until you set
`flags_with_config: true` in `src/modules/brocanth.rs`.

## Examples

### Validate a struct

```rust
use brocanth_schema::{SchemaNode, Validate};

#[derive(Validate)]
struct SignUp {
    #[validate(string(min_length = 3, max_length = 32))]
    username: String,
    #[validate(string(email))]
    email: String,
    #[validate(int(min = 13, max = 130))]
    age: i64,
    #[validate(array(min_items = 1, max_items = 5, unique), each(string(non_empty)))]
    tags: Vec<String>,
}

let bad = SignUp {
    username: "ab".into(),
    email: "nope".into(),
    age: 9,
    tags: vec![String::new()],
};

// Every issue, not only the first.
let issues = bad.validate().unwrap_err();
assert_eq!(issues.paths(), vec!["username", "email", "age", "tags[0]"]);
assert_eq!(issues.messages_at("email"), vec!["must be an email address"]);

// The description that OpenAPI and the admin read.
assert!(matches!(SignUp::schema(), SchemaNode::Object(_)));
```

A constraint that cannot apply to the field's type does not compile. For
example, `#[validate(string(min_length = 3))]` on an `i64` field is a
build error. The crate docs list each case as a `compile_fail` test.

### Validate data that has no struct

```rust
use brocanth_schema::{int, object, string, Schema, Value};

let schema = object()
    .field("email", string().email())
    .field("age", int().min(13));

let good = Value::object([("email", Value::text("a@b.co")), ("age", Value::Int(30))]);
assert!(schema.validate(&good).is_ok());

let bad = Value::object([("email", Value::text("nope")), ("age", Value::Int(9))]);
let issues = schema.validate(&bad).unwrap_err();
assert_eq!(issues.paths(), vec!["email", "age"]);

// The body of a 422 response.
assert!(issues.to_json().starts_with("{\"error\":\"validation_failed\""));
```

### Refine and transform

```rust
use brocanth_schema::{string, Schema};

let slug = string()
    .min_length(3)
    .refine("no_spaces", "must not contain a space", |s: &str| !s.contains(' '))
    .transform(|s: String| s.to_lowercase());

assert_eq!(slug.parse("Hello").unwrap(), "hello");
assert_eq!(
    slug.parse("a b").unwrap_err().messages_at(""),
    vec!["must not contain a space"]
);
```

### Carry the proof into a handler

`Valid<T>` has one constructor: `Validation::check`. `Validation` is a
context item that `ValidationPlugin` provides. If the application does not
register the plugin, `cx.extract()` does not compile.

```rust
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_schema::{Valid, Validate, Validation, ValidationConfig, ValidationPlugin};

#[derive(Debug, Validate)]
struct SignUp {
    #[validate(string(email))]
    email: String,
}

// The handler asks for proof, not for a body.
fn create(body: Valid<SignUp>) -> String {
    body.into_inner().email
}

let app = AppBuilder::new()
    .plugin(ValidationPlugin::new(ValidationConfig::default()))
    .build()
    .unwrap();
let cx = app.context();
let validation: &Validation = cx.extract();

let good = validation.check(SignUp { email: "a@b.co".into() }).unwrap();
assert_eq!(create(good), "a@b.co");

let issues = validation.check(SignUp { email: "nope".into() }).unwrap_err();
// `render` follows the `report` setting: JSON by default.
assert!(validation.render(&issues).contains("\"path\":\"email\""));
```

## Testing

```sh
cargo test -p brocanth-schema
```

No database and no environment variable are needed. The tests cover the
vocabulary (`tests/vocabulary.rs`), the derive grammar (`tests/derive.rs`)
and the feature gates (`tests/modularity.rs`). Every Rust block in this
README runs as a doctest.

In an application, test a schema directly: call `validate()` and assert on
`Issues::paths()` and `Issues::messages_at()`. To test a screen that
validates a form, use `brocanth-test`'s `TestClient` and assert on the
messages the page shows.

## Links

- `docs/input.md`: `brocanth-schema` beside `brocanth-input`, and which one
  to use.
- `docs/module-flags.md`: how the `report` setting becomes a key, a flag
  and a variable.
- The crate docs (`cargo doc -p brocanth-schema --open`): the full
  vocabulary and what does not compile.
