//! Translations, checked when the application builds.
//!
//! # The shape
//!
//! A catalog is one TOML file per locale, `locales/en.toml`,
//! `locales/fr.toml`, `locales/fr-CA.toml`. The build script compiles the
//! directory into Rust: an enum with one variant per file, and one function per
//! key.
//!
//! ```toml
//! # locales/en.toml
//! title = "Your cart"
//! greeting = "Hello, {name}!"
//!
//! [cart_items]
//! "=0" = "Your cart is empty"
//! one = "{count} item"
//! other = "{count} items"
//! ```
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     brocanth_i18n::build::compile("locales", "en").unwrap_or_else(|e| panic!("{e}"));
//! }
//!
//! // src/i18n.rs
//! include!(concat!(env!("OUT_DIR"), "/i18n.rs"));
//!
//! // a handler, an rsx template, a jt template: the same call
//! t::cart_items(locale, 3)          // "3 items", "3 articles"
//! t::greeting(locale, &user.name)   // "Hello, Max!"
//! ```
//!
//! # What fails the build
//!
//! - A key that a translation has and the default locale does not.
//! - A placeholder that one locale has and another does not, or that has a
//!   different type in each: `{when:date}` in one and `{when}` in the other.
//! - A plural message with no `other`, or a plural key CLDR does not define.
//! - A key that cannot be a Rust function name.
//!
//! The compiler checks the rest, because the keys are functions: a
//! misspelled key, a missing argument and an argument of the wrong type are
//! all rustc errors at the call site. A key that a translation **lacks** is
//! not an error. That locale falls back to its parent (`fr-CA` to `fr`), and
//! then to the default, and the build prints a warning naming the key.
//!
//! # Why TOML and not Fluent
//!
//! Fluent is the better language for translators who need selectors on
//! gender or case. It is also a runtime: `fluent-bundle` parses `.ftl` at
//! start-up, and a missing message is a `None` at request time. What this
//! crate needs from a format is a key, a string with named holes, and CLDR's
//! plural categories, all checked before the binary exists. TOML says that in
//! a syntax every brocanth project already reads (`brocanth-config`), with
//! the same parser and spans for the error messages. ICU MessageFormat's
//! `=0` exact match is borrowed for the one case CLDR categories cannot say.
//!
//! # Locale negotiation
//!
//! [`Negotiator`] picks a locale for a request: an explicit query parameter,
//! then a cookie, then `Accept-Language`, then the default. Each candidate is
//! matched exactly, then with its subtags removed (`fr-CH` to `fr`). With the
//! `plugin` feature, [`I18n`] puts a `Negotiator` on the context, where a
//! handler reads it with `ctx.provided::<Negotiator<Locale>, _>()`, and
//! [`I18nHook`] negotiates in a request hook and writes `Content-Language`
//! and `Vary` on the response.
//!
//! # Features
//!
//! | Feature | What it adds |
//! |---|---|
//! | `runtime` (default) | [`Formats`]: CLDR plural rules and number formatting |
//! | `dates` | date formatting, the largest block of CLDR data |
//! | `plugin` | [`I18n`] and [`I18nHook`], the brocanth plugins |
//! | `build` | [`build`], the catalog compiler, for a build script |

mod negotiate;

pub use negotiate::{Asked, Locales, Negotiated, Negotiator, NegotiatorConfig, Source};

#[cfg(feature = "runtime")]
mod format;

#[cfg(feature = "runtime")]
pub use format::{Date, DateLength, Formats, Plural};

#[cfg(feature = "plugin")]
mod plugin;

#[cfg(feature = "plugin")]
pub use plugin::{I18n, I18nHook};

#[cfg(feature = "build")]
pub mod build;
