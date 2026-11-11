//! Each module's manifest names exactly the hooks its plugin registers.
//!
//! A project's module file has one field per hook that a project may turn
//! off, and its header names the others. The generated `hooks_off_from`
//! passes the names that are off to the core. A hook that the plugin
//! registers and the manifest forgets is in neither place, and no file can
//! turn it off. A name that the manifest has and the plugin does not stops
//! the project's build.
//!
//! One check for every module, so a new module cannot be left out:
//!
//! * Each module in the CLI's list that declares a hook must have a row in
//!   [`PLUGINS`].
//! * Each plugin that a golden project registers with `plugin_hooking` must
//!   have a row too, so a module that hooks and declares nothing fails.
//! * For each row, the names the plugin registers equal the names the
//!   manifest declares. A plugin's `HookSource::HOOKS`, when not empty, must
//!   equal them too.
//!
//! The plugin of a row lives in its crate, or in the project: `logging`,
//! `storage-routes` and `webhooks-in` write their plugin into `src/plugins/`.
//! For those, the row reads the names from the generated source. `mise run generated` builds
//! that source, so the names it reads are the names that run.

use std::collections::BTreeSet;
use std::path::Path;

use brocanth::plugin_api::Plugin;
use brocanth::plugin_hooks::{hook_names, HookSource};

/// One hooking plugin: the module key, the plugin type as a generated
/// project names it, and the names it registers.
struct Row {
    key: &'static str,
    ty: &'static str,
    registered: fn() -> Vec<String>,
}

/// Every hooking plugin of a module the CLI has. Each is configured so that
/// every hook installs.
const PLUGINS: &[Row] = &[
    Row {
        key: "analytics",
        ty: "brocanth_analytics::Analytics",
        registered: || {
            names(&brocanth_analytics::Analytics::new(
                brocanth_analytics::AnalyticsConfig {
                    enabled: true,
                    collector_origins: vec!["https://example.com".to_string()],
                    ..Default::default()
                },
            ))
        },
    },
    Row {
        key: "beacon",
        ty: "brocanth_analytics::Beacon",
        registered: || {
            names(&brocanth_analytics::Beacon::new(
                brocanth_analytics::BeaconConfig {
                    collector: Some("https://app.example.com/_a/v".to_string()),
                    ..Default::default()
                },
            ))
        },
    },
    Row {
        key: "audit",
        ty: "brocanth_audit::AuditPlugin",
        registered: || names(&brocanth_audit::AuditPlugin::new(Default::default())),
    },
    Row {
        key: "errors",
        ty: "brocanth_errors::Errors",
        registered: || names(&brocanth_errors::Errors::new(Default::default())),
    },
    Row {
        key: "gtag",
        ty: "brocanth_gtag::Gtag",
        registered: || {
            names(&brocanth_gtag::Gtag::new(brocanth_gtag::GtagConfig {
                consent_log: Some("log/consent.jsonl".to_string()),
                ..Default::default()
            }))
        },
    },
    Row {
        key: "i18n",
        // An alias for `brocanth_i18n::I18nHook<Locale>`, where the build
        // script writes `Locale`.
        ty: "crate::plugins::i18n::I18n",
        registered: || {
            names(&brocanth_i18n::I18nHook::<OneLocale>::new(
                Default::default(),
            ))
        },
    },
    Row {
        key: "middleware-csrf",
        ty: "brocanth_middleware::Csrf",
        registered: || names(&brocanth_middleware::Csrf::new(Default::default())),
    },
    Row {
        key: "middleware-security-headers",
        ty: "brocanth_middleware::SecurityHeaders",
        registered: || {
            names(&brocanth_middleware::SecurityHeaders::new(
                Default::default(),
            ))
        },
    },
    Row {
        key: "middleware-rate-limit",
        ty: "brocanth_middleware::RateLimit",
        registered: || names(&brocanth_middleware::RateLimit::new(Default::default())),
    },
    Row {
        key: "middleware-conditional-get",
        ty: "brocanth_middleware::ConditionalGet",
        registered: || names(&brocanth_middleware::ConditionalGet::new(Default::default())),
    },
    Row {
        key: "middleware-compression",
        ty: "brocanth_middleware::Compression",
        registered: || names(&brocanth_middleware::Compression::new(Default::default())),
    },
    Row {
        key: "flash",
        ty: "brocanth_middleware::FlashPlugin",
        registered: || names(&brocanth_middleware::FlashPlugin::new(Default::default())),
    },
    Row {
        key: "logging",
        ty: "crate::plugins::logging::Logging",
        registered: || written(&["logging"], "src/plugins/logging/mod.rs"),
    },
    Row {
        key: "storage-routes",
        ty: "crate::plugins::storage_routes::StorageRoutes",
        registered: || {
            written(
                &["config", "storage", "storage-routes"],
                "src/plugins/storage_routes/mod.rs",
            )
        },
    },
    Row {
        key: "webhooks-in",
        ty: "crate::plugins::webhooks_in::WebhooksIn",
        registered: || {
            written(
                &["config", "webhooks", "webhooks-in"],
                "src/plugins/webhooks_in/mod.rs",
            )
        },
    },
];

/// The names `plugin` registers, and its `HOOKS` when it lists them: both
/// must equal the manifest's.
fn names<P: HookSource>(plugin: &P) -> Vec<String> {
    let registered = hook_names(plugin).unwrap_or_else(|e| panic!("{}: {e}", P::NAME));
    if !P::HOOKS.is_empty() {
        let listed: BTreeSet<&str> = P::HOOKS.iter().copied().collect();
        let installed: BTreeSet<&str> = registered.iter().map(String::as_str).collect();
        assert_eq!(
            listed,
            installed,
            "`{}`: `HookSource::HOOKS` differs from the hooks a full configuration installs\n\
             help: list in `HOOKS` every name `install_hooks` can register",
            P::NAME
        );
    }
    registered
}

/// The names a generated plugin passes to its registrar: the first string
/// after each `registrar.<method>(`.
fn written(picks: &[&'static str], path: &str) -> Vec<String> {
    let files =
        brocanth_cli::testing::generate("my-blog", picks, &[]).unwrap_or_else(|e| panic!("{e}"));
    let source = &files
        .iter()
        .find(|(p, _)| p == path)
        .unwrap_or_else(|| panic!("the project with {picks:?} has no {path}"))
        .1;
    let mut found = Vec::new();
    for (at, _) in source.match_indices("registrar.") {
        let call = &source[at..];
        let Some(open) = call.find('(') else { continue };
        let args = call[open + 1..].trim_start();
        if let Some(rest) = args.strip_prefix('"') {
            found.push(rest[..rest.find('"').expect("a closed string")].to_string());
        }
    }
    assert!(
        !found.is_empty(),
        "{path}: no `registrar.<method>(\"name\", ..)` call\n\
         help: give the hook name as the first argument, as a string literal"
    );
    found
}

/// A project's `Locale` with one locale, for the `i18n` plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OneLocale;

impl brocanth_i18n::Locales for OneLocale {
    const ALL: &'static [Self] = &[OneLocale];
    const DEFAULT: Self = OneLocale;

    fn tag(self) -> &'static str {
        "en"
    }

    fn parent(self) -> Option<Self> {
        None
    }
}

fn declared(module: &dyn brocanth_scaffold::Module) -> BTreeSet<String> {
    module
        .manifest()
        .hooks
        .iter()
        .map(|(name, _, _)| name.to_string())
        .collect()
}

#[test]
fn every_module_that_declares_a_hook_has_a_row() {
    let missing: Vec<&str> = brocanth_cli::testing::modules()
        .iter()
        .map(|m| m.manifest())
        .filter(|m| !m.hooks.is_empty())
        .map(|m| m.key)
        .filter(|key| !PLUGINS.iter().any(|row| row.key == *key))
        .collect();
    assert!(
        missing.is_empty(),
        "these modules declare hooks and have no row in PLUGINS: {missing:?}\n\
         help: add a row in crates/brocanth-cli/tests/manifest_hooks.rs that builds the plugin"
    );
}

#[test]
fn every_row_names_a_module_the_cli_has() {
    let keys: Vec<&str> = brocanth_cli::testing::modules()
        .iter()
        .map(|m| m.manifest().key)
        .collect();
    for row in PLUGINS {
        assert!(
            keys.contains(&row.key),
            "PLUGINS has `{}`, which no module has\n\
             help: correct the key, or remove the row with the module",
            row.key
        );
    }
}

/// The types after `plugin_hooking(<`, `plugin_hooking_from_config::<` and
/// `plugin_hooking_from_config_with::<` in every golden project.
fn hooking_types() -> BTreeSet<String> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut types = BTreeSet::new();
    for krate in std::fs::read_dir(&crates).expect("crates/") {
        let golden = krate.expect("a crate").path().join("tests/golden");
        let Ok(files) = std::fs::read_dir(&golden) else {
            continue;
        };
        for file in files {
            let text = std::fs::read_to_string(file.expect("a golden").path()).unwrap_or_default();
            for marker in [
                "plugin_hooking(",
                "plugin_hooking_from_config::",
                "plugin_hooking_from_config_with::",
            ] {
                for (at, _) in text.match_indices(marker) {
                    // rustfmt can break the line between `(` and `<`. A
                    // comment there is a suggestion, not a registration.
                    let Some(rest) = text[at + marker.len()..].trim_start().strip_prefix('<')
                    else {
                        continue;
                    };
                    let end = rest
                        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
                        .unwrap_or(rest.len());
                    types.insert(rest[..end].to_string());
                }
            }
        }
    }
    types
}

#[test]
fn every_plugin_a_golden_project_hooks_has_a_row() {
    let types = hooking_types();
    assert!(
        types.contains("brocanth_gtag::Gtag"),
        "the goldens register no hooking plugin: {types:?}"
    );
    let missing: Vec<&String> = types
        .iter()
        .filter(|ty| !PLUGINS.iter().any(|row| row.ty == ty.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "a golden project hooks these plugins, and PLUGINS has no row for them: {missing:?}\n\
         help: declare the plugin's hooks in its manifest with `Manifest::hook`, and add a row \
         in crates/brocanth-cli/tests/manifest_hooks.rs"
    );
}

#[test]
fn every_manifest_declares_exactly_the_hooks_its_plugin_registers() {
    let mut wrong = Vec::new();
    for row in PLUGINS {
        let module = brocanth_cli::testing::modules()
            .iter()
            .find(|m| m.manifest().key == row.key)
            .unwrap_or_else(|| panic!("no module `{}`", row.key));
        let declared = declared(*module);
        let registered: BTreeSet<String> = (row.registered)().into_iter().collect();
        if declared != registered {
            wrong.push(format!(
                "`{}`: the plugin registers {registered:?}, the manifest declares {declared:?}",
                row.key
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{}\nhelp: make `Manifest::hook` list each name the plugin passes to its registrar",
        wrong.join("\n")
    );
}
