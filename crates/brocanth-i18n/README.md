# brocanth-i18n

Translations that the build checks. A catalog is one TOML file per locale.
A build script compiles the directory into Rust: a `Locale` enum with one
variant per file, and a module `t` with one function per key. A misspelt
key, a missing argument and a wrong argument type are rustc errors at the
call site. The crate also negotiates a locale for a request (query, cookie,
`Accept-Language`), and formats plurals, numbers and dates with CLDR data
(ICU4X). Use it for an application that shows text in more than one
language. Do not use it if you need gender or other selects, ordinal
plurals, currency formatting, or a translator workflow built on Fluent or
gettext: those are not here (see `docs/i18n.md` §7).

**Why TOML and not Fluent:** Fluent is a runtime. It parses `.ftl` at start
and a missing message is a `None` at request time. This crate needs a key,
a string with named holes, and CLDR plural categories, all checked before
the binary exists. Every brocanth project already reads TOML.

## Getting started

In a brocanth project, `brocanth add i18n` writes all of this: the two
dependency lines, `build.rs`, `locales/en.toml` and `locales/fr.toml`, and
the hook with its `[i18n]` settings (`docs/modules/i18n.md`). By hand, add
the crate twice:

```toml
[dependencies]
brocanth-i18n = { git = "https://github.com/DHHwasRight/brocanth.git", features = ["plugin"] }

[build-dependencies]
brocanth-i18n = { git = "https://github.com/DHHwasRight/brocanth.git", default-features = false, features = ["build"] }
```

**Reason for two lines:** the build script links a TOML parser and no CLDR
data. The binary links CLDR data and no TOML parser. Resolver 2 keeps the
two feature sets apart.

| Feature | Default | Adds |
| --- | --- | --- |
| `runtime` | on | `Formats`: CLDR plural rules and number formatting |
| `dates` | off | Date formatting. Separate, because the date data is the largest part of ICU4X. |
| `plugin` | off | `I18n` and `I18nHook`, the brocanth plugins |
| `build` | off | `build::compile`, the catalog compiler, for a build script |
| `config` | off | `NegotiatorConfig` deserialises from an `[i18n]` section |
| `scaffold` | off | the `i18n` module, for the `brocanth` CLI |

Then write the catalog, the build script and the include:

```toml
# locales/en.toml
title = "Your cart"
greeting = "Hello, {name}!"
total = "Total: {amount:number}"

[cart_items]
"=0" = "Your cart is empty"
one = "{count} item"
other = "{count} items"
```

```rust,no_run
// build.rs
fn main() {
    brocanth_i18n::build::compile("locales", "en").unwrap_or_else(|e| panic!("{e}"));
}
```

```text
// src/i18n.rs
include!(concat!(env!("OUT_DIR"), "/i18n.rs"));
```

The `include!` block is text, not a doctest: the file it names exists only
after a build script runs.

## Examples

The examples below use the crate's own test catalogs
(`tests/locales/en.toml`, `fr.toml`, `fr-CA.toml`) and the file the build
writes from them (`tests/golden/i18n.rs`).

### Call a key

```rust
# #[allow(dead_code)]
# mod i18n { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/i18n.rs")); }
use i18n::{t, Locale};

assert_eq!(t::title(Locale::Fr), "Votre panier");
assert_eq!(t::greeting(Locale::En, "Max"), "Hello, Max!");

// Plurals follow CLDR per language; "=0" is an exact match.
assert_eq!(t::cart_items(Locale::En, 0), "Your cart is empty");
assert_eq!(t::cart_items(Locale::En, 1), "1 item");
assert_eq!(t::cart_items(Locale::Fr, 3), "3 articles");

// fr-CA has no `greeting`: it falls back to fr, then to the default.
assert_eq!(t::greeting(Locale::FrCa, "Max"), "Bonjour, Max !");
```

The same call works in a handler, an `.rsx` template (`{t::title(locale)}`)
and a `jt` template. rsx escapes the text as HTML; jt writes a JSON string.

### What fails the build

`build::generate` is `compile` without the file system. It reports every
problem at once, with file and line.

```rust
use brocanth_i18n::build::{generate, Catalog};

let catalog = |tag: &str, text: &str| Catalog {
    tag: tag.to_string(),
    path: format!("locales/{tag}.toml"),
    text: text.to_string(),
};

// fr has a key the default does not have.
let err = generate(
    &[
        catalog("en", "title = \"Your cart\"\n"),
        catalog("fr", "title = \"Votre panier\"\nextra = \"Bonus\"\n"),
    ],
    "en",
)
.unwrap_err();
assert!(err.to_string().contains("extra"));

// A key a translation lacks is a warning, not an error.
let ok = generate(
    &[
        catalog("en", "title = \"Your cart\"\nhome = \"Home\"\n"),
        catalog("fr", "title = \"Votre panier\"\n"),
    ],
    "en",
)
.unwrap();
assert!(ok.code.contains("pub fn home"));
assert_eq!(ok.warnings.len(), 1);
```

### Negotiate a locale

`Negotiator` reads the query parameter, then the cookie, then
`Accept-Language`, then gives the default. It matches each value exactly,
then with subtags removed (`fr-CH` to `fr`).

```rust
# #[allow(dead_code)]
# mod i18n { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/i18n.rs")); }
use brocanth_i18n::{Asked, Negotiator, NegotiatorConfig, Source};
use i18n::Locale;

let negotiator = Negotiator::<Locale>::new(NegotiatorConfig::default());

let picked = negotiator.negotiate(Asked {
    query: "",
    cookie: None,
    accept_language: Some("fr-CH, en;q=0.5"),
});
assert_eq!((picked.locale, picked.source), (Locale::Fr, Source::AcceptLanguage));

let picked = negotiator.negotiate(Asked {
    query: "page=2&locale=en",
    cookie: Some("locale=fr"),
    accept_language: Some("fr"),
});
assert_eq!((picked.locale, picked.source), (Locale::En, Source::Query));
```

### Register the plugin

`I18n` puts a `Negotiator<Locale>` on the context; a handler reads it with
`ctx.provided::<Negotiator<Locale>, _>()`. `I18nHook` negotiates in a
request hook and writes `Content-Language` and `Vary` on the response.
Both refuse a query or cookie name that a client cannot send.

```rust
# #[allow(dead_code)]
# mod i18n { include!(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/i18n.rs")); }
use brocanth::plugin_api::Plugin;
use brocanth::plugin_app::AppBuilder;
use brocanth_i18n::{I18n, I18nHook, NegotiatorConfig};
use i18n::Locale;

let app = AppBuilder::new()
    .plugin(I18n::<Locale>::new(NegotiatorConfig::default()))
    .build();
assert!(app.is_ok());

let hooked = AppBuilder::new()
    .plugin_hooking(I18nHook::<Locale>::new(NegotiatorConfig::default()))
    .build();
assert!(hooked.is_ok());
```

### Format numbers and dates

`Locale::formats()` gives the CLDR data for one locale, built once and kept
in a `static`. `Formats::new(tag)` builds it by hand.

```rust
use brocanth_i18n::{Date, DateLength, Formats};

let en = Formats::new("en");
let fr = Formats::new("fr");
assert_eq!(en.number(1_234_567), "1,234,567");
assert_eq!(en.fixed(1050, 2), "10.50");
assert_eq!(fr.fixed(1050, 2), "10,50");

let day = Date::new(2026, 10, 30).unwrap();
assert_eq!(en.date(day, DateLength::Long), "October 30, 2026");
```

`date` needs the `dates` feature.

## Testing

```sh
cargo test -p brocanth-i18n
```

No environment variable is needed. The crate takes itself as a
dev-dependency with every feature, so no test is skipped. `tests/golden.rs`
checks that `tests/golden/i18n.rs` is what the build writes from
`tests/locales/`; `tests/build_errors.rs` checks each build error;
`tests/templates.rs` renders a key in rsx and jt.

An application tests its translations by calling `t::*` with each
`Locale`, as above. To test what a page shows in a language, send the
request through `brocanth-test`'s `TestClient` with an `accept-language`
header (`TestRequest::get(..).header("accept-language", "fr")`) and assert
on the text with the `Html` reader, not on CSS classes. See
`crates/brocanth-test/README.md`.

## Links

- `docs/i18n.md`: the catalog syntax, every build error, negotiation, the
  plugins, templates, formatting, and what is open.
- `tests/golden/i18n.rs`: a complete generated file.
