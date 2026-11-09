//! `i18n`, as a choice on the `brocanth create` menu.
//!
//! What a project gets:
//!
//! - `locales/en.toml` and `locales/fr.toml`, the catalogs. English is the
//!   default and has every key. French is there so that negotiation has
//!   something to choose from on the first run.
//! - `build.rs`, which compiles `locales/` into `$OUT_DIR/i18n.rs`. A
//!   catalog error stops the build with its file, line and column.
//! - `src/plugins/i18n/mod.rs`, which includes the generated code, names the
//!   plugin, and gives a page the locale: `i18n::locale(ctx)`.
//! - [`I18nHook`](crate::I18nHook), registered with its settings. It
//!   negotiates before the page runs and stamps `Content-Language`.
//!
//! It is `outside_runtime`: the static export runs every page through the
//! hooks, so a static site gets the catalogs, the plurals and the number
//! formats. It gets one language, the `default` setting's, because an
//! export sends no `Accept-Language` and a file cannot vary by request.

use brocanth_scaffold::snippet::{configurable_hooking, Hooking};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Translations checked at build time, and a locale per request.
pub struct I18n;

impl Module for I18n {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "i18n",
            "translations checked at build time; a locale per request from the query, a cookie or Accept-Language",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "gtag",
            "the catalogs carry the consent banner's strings, so the banner follows the locale",
        )
        .adapts_to(
            "rsx",
            "AGENTS.md says how an `.rsx` page takes the locale and sets `lang`",
        )
        .adapts_to(
            "jt",
            "AGENTS.md says how a `.jt` template writes a translation",
        )
        // The export runs every page through the hooks, so a static site
        // takes it: one language per export, the `default` setting's.
        .outside_runtime()
        .asked_in("pages", "Show the site in more than one language")
        .krate(
            Dep::brocanth("brocanth-i18n")
                .feature("plugin")
                .feature_with("config", "config"),
        )
        .krate(
            Dep::brocanth("brocanth-i18n")
                .no_default_features()
                .feature("build")
                .for_build()
                .note("build.rs compiles locales/; no CLDR data in the build script"),
        )
        .absent(
            "brocanth-i18n",
            "one language, written into each page; nothing negotiates a locale",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let gtag = project.has("gtag");
        s.file("build.rs", BUILD_RS);
        s.file(
            "locales/en.toml",
            format!("{EN}{}", if gtag { GTAG_EN } else { "" }),
        );
        s.file(
            "locales/fr.toml",
            format!("{FR}{}", if gtag { GTAG_FR } else { "" }),
        );
        s.file("src/plugins/i18n/mod.rs", MOD_RS);
        s.at(Slot::PluginsMod).once("pub mod i18n;");

        let turn = if project.has("config") {
            "under [i18n] in config/<env>.toml"
        } else {
            "with `--i18n.default=fr` and the flags `--help` lists"
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "i18n",
                section: "i18n",
                ty: "crate::plugins::i18n::I18n",
                config_ty: "brocanth_i18n::NegotiatorConfig",
                prelude: PRELUDE.replace("@TURN@", turn),
                adjust: None,
            },
            vec![
                Setting::text(
                    "query",
                    "locale",
                    "The query parameter that names a locale: `?locale=fr`.",
                )
                .optional(),
                Setting::text(
                    "cookie",
                    "locale",
                    "The cookie that names a locale.",
                )
                .optional(),
                Setting::text(
                    "default",
                    "",
                    "The locale for a request that asks for none offered. None: `en`, the catalog's default.",
                )
                .note("A static export sends no Accept-Language, so this is the language of every exported page.")
                .optional(),
                Setting::list(
                    "available",
                    &[],
                    "The locales a request may get, by tag. Empty: every file in locales/.",
                )
                .note("A catalog left out still compiles, so a translation in progress ships unseen."),
                Setting::bool(
                    "remember",
                    true,
                    "A `?locale=` choice sets the cookie, so the next page keeps it.",
                ),
            ],
        );

        let mut agents = AGENTS.to_string();
        if project.has("rsx") {
            agents.push_str(AGENTS_RSX);
        }
        if project.has("jt") {
            agents.push_str(AGENTS_JT);
        }
        if gtag {
            agents.push_str(AGENTS_GTAG);
        }
        s.at(Slot::AgentsSections).block(agents);
    }
}

const BUILD_RS: &str = r#"//! Written by `brocanth`. Compiles `locales/*.toml` into `$OUT_DIR/i18n.rs`,
//! which `src/plugins/i18n/mod.rs` includes.
//!
//! The second argument is the default catalog: every other one is checked
//! against it, and a key it lacks is a build error. A key a translation
//! lacks is a warning, and that locale shows the default's text.

fn main() {
    brocanth_i18n::build::compile("locales", "en").unwrap_or_else(|e| panic!("{e}"));
}
"#;

const EN: &str = r#"# The default catalog. Every key is here; `build.rs` names this file.
#
# A key is a function: `welcome` is `t::welcome(locale)`. `{name}` is an
# argument, and a table with `one` and `other` is a plural. A misspelt key
# or a missing argument is a compile error at the call. See the
# `brocanth-i18n` README.

welcome = "Welcome"
greeting = "Hello, {name}!"

[items]
"=0" = "No items"
one = "{count} item"
other = "{count} items"
"#;

const FR: &str = r#"# French. A key left out here shows the English text, and the build
# says which.

welcome = "Bienvenue"
greeting = "Bonjour, {name} !"

[items]
"=0" = "Aucun article"
one = "{count} article"
other = "{count} articles"
"#;

const GTAG_EN: &str = r#"
# The consent banner of the `gtag` module. Its hook reads these keys in the
# request's locale. A key left out of every catalog falls back to the
# `[gtag.banner]` setting.
[gtag.banner]
text = "We would like to measure visits with Google Analytics. It sets cookies only if you accept."
accept = "Accept"
refuse = "Refuse"
manage = "Cookie settings"
policy = "Privacy policy"
"#;

const GTAG_FR: &str = r#"
[gtag.banner]
text = "Nous aimerions mesurer les visites avec Google Analytics. Il ne dépose de cookies que si vous acceptez."
accept = "Accepter"
refuse = "Refuser"
manage = "Gérer les cookies"
policy = "Politique de confidentialité"
"#;

const MOD_RS: &str = r#"//! Written by `brocanth`. The catalogs in `locales/`, compiled by
//! `build.rs`, and the locale of a request.
//!
//! A page reads the locale and calls a key:
//!
//! ```ignore
//! use crate::plugins::i18n::{self, t};
//!
//! let locale = i18n::locale(ctx);
//! let title = t::welcome(locale);
//! ```

use brocanth_i18n::{Locales, Negotiated};

use crate::pages::Ctx;

// A key that no code calls yet is not a warning: a new project calls
// none. Remove the `allow` lines here to find what nothing uses.
#[allow(dead_code)]
mod catalog {
    include!(concat!(env!("OUT_DIR"), "/i18n.rs"));
}

#[allow(unused_imports)]
pub use catalog::t;
pub use catalog::Locale;

/// The plugin `src/plugins/mod.rs` registers. It negotiates in a request
/// hook, before the page runs.
pub type I18n = brocanth_i18n::I18nHook<Locale>;

/// The locale the `i18n` hook chose for this request.
///
/// The hook reads `?locale=`, then the cookie, then `Accept-Language`,
/// then the `default` setting. The fallback here is only for a context
/// the hook did not see, such as a test that builds a `Ctx` by hand.
#[allow(dead_code)]
pub fn locale(ctx: &Ctx) -> Locale {
    ctx.hooked::<Negotiated<Locale>>()
        .map_or(Locale::DEFAULT, |negotiated| negotiated.locale)
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    use super::{t, Locale};

    async fn get(request: Request) -> brocanth_server::Response {
        let app = crate::app::build().expect("the application builds");
        crate::app::handle(&app, request).await
    }

    #[tokio::test]
    async fn a_page_answers_in_the_language_the_browser_asks_for() {
        let french = Request::new(Verb::Get, "/").with_header("accept-language", "fr-CH");
        let response = get(french).await;
        assert_eq!(response.header("content-language"), Some("fr"));
        let response = get(Request::new(Verb::Get, "/")).await;
        assert_eq!(response.header("content-language"), Some("en"));
    }

    #[tokio::test]
    async fn a_link_with_a_locale_wins_and_is_remembered() {
        let mut request = Request::new(Verb::Get, "/").with_header("accept-language", "en");
        request.query = "locale=fr".to_string();
        let response = get(request).await;
        assert_eq!(response.header("content-language"), Some("fr"));
        // Another module (csrf) may set a cookie of its own first.
        let cookies: Vec<&str> = response
            .headers()
            .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, value)| value)
            .collect();
        assert!(
            cookies.iter().any(|c| c.starts_with("locale=fr;")),
            "{cookies:?}"
        );
    }

    #[test]
    fn every_locale_has_the_keys_with_its_own_plural_rules() {
        assert_eq!(t::welcome(Locale::Fr), "Bienvenue");
        assert_eq!(t::items(Locale::En, 0), "No items");
        assert_eq!(t::items(Locale::En, 1), "1 item");
        assert_eq!(t::items(Locale::Fr, 1), "1 article");
        assert_eq!(t::greeting(Locale::En, "Ada"), "Hello, Ada!");
    }
}
"#;

/// The comment above the registration. `@TURN@` says where the settings
/// are, which depends on whether there is a file.
const PRELUDE: &str = r#"
        // -- brocanth-i18n: a locale per request --------------------------
        //
        // Reads `?locale=`, then the `locale` cookie, then Accept-Language,
        // and gives the page the locale (`i18n::locale(ctx)`). Says
        // Content-Language and Vary on the way out. The default locale and
        // the offered ones are settings, @TURN@.
"#;

const AGENTS: &str = "\n\
    ## Translations\n\
    \n\
    Text a visitor reads lives in `locales/<tag>.toml`, one file per\n\
    locale. `locales/en.toml` is the default and has every key. `build.rs`\n\
    compiles the files: each key is a function, `t::welcome(locale)`, so a\n\
    misspelt key or a missing argument does not compile. Do not write\n\
    visible text as a literal in a page: add the key to `en.toml`, then to\n\
    each translation. A key a translation lacks is a build warning, and\n\
    that locale shows the English text.\n\
    \n\
    A page reads the locale with `crate::plugins::i18n::locale(ctx)`. The\n\
    hook in `src/plugins/i18n/mod.rs` chose it from `?locale=`, the\n\
    `locale` cookie, then `Accept-Language`. A link with `?locale=fr` is a\n\
    language switcher: the hook remembers the choice in the cookie.\n\
    \n\
    A hook in another crate cannot name `Locale`. It reads\n\
    `brocanth_i18n::Current` from the request state and calls\n\
    `current.text(\"key.path\")`, which answers keys with no argument.\n";

const AGENTS_RSX: &str = "\n\
    In an `.rsx` page, take the locale as a parameter (`locale: Locale`)\n\
    and write `{t::welcome(locale)}`: rsx escapes it like any value. The\n\
    layout cannot see the request, so it says `lang=\"en\"`; put\n\
    `lang={locale}` on the page's own root element.\n";

const AGENTS_JT: &str = "\n\
    In a `.jt` template, `\"title\": t::welcome(locale)` writes the\n\
    translation as a JSON string.\n";

const AGENTS_GTAG: &str = "\n\
    The consent banner's text is the `[gtag.banner]` table of each catalog.\n\
    The `banner.*` settings of `gtag` are the fallback for a key no catalog\n\
    has. Change the wording in the catalogs.\n";
