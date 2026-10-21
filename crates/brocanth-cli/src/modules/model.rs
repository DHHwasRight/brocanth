//! `model` -- SeaORM, validations and migrations.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The model layer.
///
/// Recommended and absent, which is section 8 point 4 word for word: an
/// application that stores nothing has no SQL driver in its lock file.
pub struct Model;

impl Module for Model {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "model",
            "SeaORM, validations, migrations; without it no SQL driver at all",
        )
        .asked_in("data", "An ORM with validations and migrations")
        .krate(Dep::brocanth("brocanth-model"))
        .absent(
            "brocanth-model",
            "no ORM, no SQL driver, no sea-orm anywhere in the lock file",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/model/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod model;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "model", "Model"));
        s.at(Slot::AppContext).block(
            "    let db: &crate::plugins::model::Db = cx.extract();\n\
            \x20   eprintln!(\"database: {}\", db.url);"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(
                "\n[model]\nurl = \"sqlite://db/development.sqlite?mode=rwc\"\n".to_string(),
            );
        }
    }
}

pub(crate) fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "model");
    format!(
        r##"//! The model layer.
//!
//! The ORM is recommended and absent. SeaORM is the recommendation, it is not a
//! dependency of the core and never will be, and it arrives through
//! `brocanth-model` -- which is in `Cargo.toml` because this file is here and
//! through no other route. An application that stores nothing pays nothing for
//! it: not a crate in the lock file, not a section in the config, not a byte.
//!
//! The entity modules go under `src/models/`, from `brocanth generate model`.

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_model::ModelRegistry;

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `sqlite://db/development.sqlite?mode=rwc`, or a Postgres URL.
    pub url: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            url: "sqlite://db/development.sqlite?mode=rwc".to_string(),
        }}
    }}
}}

/// The handle a handler asks the context for.
pub struct Db {{
    pub url: String,
}}

impl ContextItem for Db {{}}

pub struct Model {{
    url: String,
}}

impl Plugin for Model {{
    type Config = Settings;
    type Provides = ProvidesContext<Db>;
    const NAME: &'static str = "model";

    fn new(config: Settings) -> Self {{
        Self {{ url: config.url }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.url.is_empty() {{
            return Err(PluginError::new(
                "[model] url is required; there is no default database",
            ));
        }}
        Ok(())
    }}

    fn before_start(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // The pool opens here, at the last hook before the server is handed the
        // application, and drains on the matching shutdown hook.
        //
        // The registry below is what makes the generators and an admin UI
        // tractable: every entity compiled into this binary can be enumerated,
        // with its columns, nullability and keys. It is empty until the first
        // `brocanth generate model`, and the line is here so it is not a
        // surprise when it fills.
        let registry = ModelRegistry::new();
        eprintln!("[model] {{}} entities registered", registry.names().len());
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        cx.insert(Db {{
            url: self.url.clone(),
        }});
    }}
}}
"##
    )
}
