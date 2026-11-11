//! `gtag`, as a choice on the `brocanth create` menu.
//!
//! One hooking plugin, registered through
//! [`configurable_hooking`](brocanth_scaffold::snippet::configurable_hooking).
//! It writes no template and no route: the hook puts the banner and the
//! tag into every HTML page, so `rsx`, `jt`, a hand-written handler and the
//! static export all get it the same way.
//!
//! It is off until a deployment turns it on (`enabled`), so the generated
//! project sends nothing to Google from a developer's machine.

use brocanth_scaffold::snippet::{configurable_hooking, module_ref, Hooking};
use brocanth_scaffold::{Category, Dep, Hook, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Google Analytics 4 behind a consent banner.
pub struct Gtag;

impl Module for Gtag {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "gtag",
            "Google Analytics 4 behind a consent banner; nothing leaves before the visitor accepts",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "admin",
            "the admin never gets the tag, at the path src/modules/admin.rs mounts it",
        )
        .integration(
            "i18n",
            "the banner's words come from the catalogs, in the request's locale",
            true,
        )
        // `consent-log` is no toggle: the `consent_log` setting decides it,
        // and the banner's script reports a choice exactly when that
        // setting names a file. A hook turned off under a set path would
        // leave the script reporting to nothing.
        .hook(
            "consent-log",
            "record each consent choice, while `consent_log` names a file",
            Hook::required(),
        )
        .hook(
            "insert",
            "put the banner and the tag into each HTML page",
            Hook::required(),
        )
        // The hook runs for the static export too, so a static site may
        // take it: the consent lives in the visitor's browser.
        .outside_runtime()
        .asked_in(
            "operations",
            "Count visits with Google Analytics, after consent",
        )
        .krate(
            Dep::brocanth("brocanth-gtag")
                .feature_with("config", "config")
                .feature_with("i18n", "i18n"),
        )
        .absent("gtag", "no Google tag; nothing on a page talks to Google")
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let turn_on = if project.has("config") {
            "`enabled = true` and `measurement_id` under\n        // [gtag] in config/production.toml"
        } else {
            "`--gtag.enabled=true` and\n        // `--gtag.measurement_id=G-…`, or the environment\n        // variables `--help` names"
        };
        let words = if project.has("i18n") {
            "\n        //\n        // The banner's words are the [gtag.banner] keys of\n        \
             // locales/*.toml, in the request's locale. The\n        \
             // `banner.*` settings are the fallback."
        } else {
            ""
        };
        let admin = if project.has("admin") {
            "\n        // The admin never gets the tag, wherever src/modules/admin.rs\n        \
             // mounts it."
        } else {
            ""
        };
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "gtag",
                section: "gtag",
                ty: "brocanth_gtag::Gtag",
                config_ty: "brocanth_gtag::GtagConfig",
                prelude: PRELUDE
                    .replace("@TURN_ON@", turn_on)
                    .replace("@ADMIN@", admin)
                    .replace("@WORDS@", words),
                adjust: adjust(project),
            },
            vec![
                Setting::bool(
                    "enabled",
                    false,
                    "Insert the consent banner and the Google tag into every HTML page.",
                )
                .note("Off outside production, so a developer's visits are not counted."),
                Setting::text(
                    "measurement_id",
                    "",
                    "The GA4 measurement id, `G-` then capitals and digits.",
                ),
                Setting::text(
                    "consent_mode",
                    "basic",
                    "`basic`: no Google script before consent. `advanced`: cookieless pings before it.",
                ),
                Setting::bool(
                    "respect_gpc",
                    true,
                    "Treat Global Privacy Control and Do Not Track as a refusal.",
                ),
                Setting::integer(
                    "consent_days",
                    182,
                    "How long a choice is kept before the banner asks again, in days.",
                )
                .note("Six months is what the CNIL recommends."),
                Setting::bool(
                    "send_title",
                    false,
                    "Send the page title. Off: the scrubbed path stands in for it.",
                ),
                Setting::list(
                    "allow_params",
                    &[
                        "utm_source",
                        "utm_medium",
                        "utm_campaign",
                        "utm_term",
                        "utm_content",
                    ],
                    "The query parameters that reach Google; every other one is removed.",
                ),
                Setting::list(
                    "exclude",
                    &[],
                    "Paths that never get the tag, each with everything under it: \"/drafts\".",
                )
                .note(if project.has("admin") {
                    "The admin never gets the tag: src/plugins/mod.rs reads its path from\n\
                     src/modules/admin.rs."
                } else {
                    ""
                }),
                Setting::text(
                    "cookie_domain",
                    "",
                    "The domain Google's cookies live on, such as `example.co.uk`; none lets the tag pick it.",
                )
                .optional(),
                Setting::text(
                    "consent_log",
                    "",
                    "A file each consent choice is appended to, such as `log/consent.jsonl`.",
                )
                .note("A server only: a static export has no one to report a choice to.")
                .optional(),
                Setting::text(
                    "banner.text",
                    "We would like to measure visits with Google Analytics. It sets cookies only if you accept.",
                    "The sentence the banner asks with.",
                ),
                Setting::text("banner.accept", "Accept", "The button that accepts."),
                Setting::text("banner.refuse", "Refuse", "The button that refuses."),
                Setting::text(
                    "banner.manage",
                    "Cookie settings",
                    "The button that opens the banner again.",
                ),
                Setting::text(
                    "banner.policy_url",
                    "",
                    "A link to the privacy policy, such as `/privacy`.",
                )
                .optional(),
                Setting::text("banner.policy", "Privacy policy", "The text of that link."),
            ],
        );
        let mut agents = AGENTS.to_string();
        if project.has("i18n") {
            agents.push_str(AGENTS_I18N);
        }
        s.at(Slot::AgentsSections).block(agents);
    }
}

/// The chain on the `[gtag]` configuration, after the file and the flags.
///
/// With `i18n`: the catalogs' words, behind `integrate.i18n`. The feature
/// stays on; the field decides at build time. With `admin`: the admin's
/// path, read from its module file and never copied into `exclude`, so a
/// moved admin needs no edit (docs/module-config.md §9).
fn adjust(project: &Project) -> Option<String> {
    let mut chain = String::new();
    if project.has("i18n") {
        chain.push_str(&format!(
            ".translate({})",
            module_ref("gtag", "integrate.i18n")
        ));
    }
    if project.has("admin") {
        chain.push_str(&format!(
            ".exclude_path({})",
            module_ref("admin", "mount_at")
        ));
    }
    (!chain.is_empty()).then_some(chain)
}

/// The comment above the registration. `@TURN_ON@` says where to turn it
/// on, which depends on whether there is a file; `@ADMIN@` names the
/// admin, when there is one.
const PRELUDE: &str = r#"
        // -- brocanth-gtag: Google Analytics, after consent ----------
        //
        // A banner on every HTML page, and Google's tag only once the
        // visitor accepts. Global Privacy Control counts as a refusal, and
        // the page URL loses every query parameter not on the allow-list.@ADMIN@
        // Off until production turns it on:
        // @TURN_ON@.@WORDS@
"#;

/// What the generated `AGENTS.md` says about analytics.
const AGENTS: &str = "\n\
    ## Google Analytics\n\
    \n\
    `brocanth_gtag::Gtag` is registered in `src/plugins/mod.rs`.\n\
    Its hook puts a consent banner and Google's tag into every HTML page,\n\
    before `</body>`. Nothing reaches Google before the visitor accepts.\n\
    \n\
    Do not add a second Google tag, and do not send a page view yourself:\n\
    the hook scrubs the URL first. Never put an email address, a name or a\n\
    user id in a URL, a page title or an event: Google's terms forbid it,\n\
    and the scrubber only catches what it can recognise. A link with\n\
    `data-consent-manage` reopens the banner.\n\
    \n\
    The choice lives in the visitor's browser. To keep a record of it on\n\
    the server, set `consent_log` to a file: each choice is one JSON line\n\
    with no address and no user agent.\n";

/// With `i18n`: where the banner's words are.
const AGENTS_I18N: &str = "\n\
    The banner speaks the request's locale. Its words are the\n\
    `[gtag.banner]` keys of `locales/*.toml`; change them there, in every\n\
    catalog. The `banner.*` settings are only the fallback.\n";
