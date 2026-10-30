//! `static`: the page router, compiled to a directory of files.
//!
//! The pages are the same pages. `export` builds the application, asks it
//! for every route that answers GET, and writes each answer to `dist/`,
//! beside every embedded asset. Any static host serves the result: GitHub
//! Pages, an S3 bucket, nginx. `serve` stays, as the preview.
//!
//! The work is in the `brocanth-static` crate, and the generated command
//! is a thin caller of it: a fix to the export reaches every project
//! through `cargo update`, not through a file each project owns. The same
//! crate exports applications that were not generated, such as the d3set
//! chart gallery. See `docs/static-export.md`.
//!
//! # Only what does not need a server
//!
//! A static site has no process behind it, so a module that works at
//! request time -- a session, a database, a queue, a probe -- has nothing to
//! run in. Choosing this module therefore refuses every module that does
//! not say `outside_runtime`: a template engine, a CSS build, the config
//! file, the command line, CI. The rule is in `create::exclusive`, so the
//! flags, the chooser and `brocanth add` refuse the same thing.
//!
//! This is a first cut of that rule. Which modules count as outside the
//! runtime is one builder call on each manifest, so the list can change
//! without a change here.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

pub struct StaticSite;

impl Module for StaticSite {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "static",
            "`export` writes every GET page and every asset to dist/; no server, so no runtime modules",
        )
        .asked_in("pages", "Compile the pages to files any static host serves")
        .needs("commands", "`export` is a subcommand of this binary")
        .krate(Dep::brocanth("brocanth-static"))
        .static_output()
        .outside_runtime()
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // `crate::assets` exists only when a module embeds files.
        let assets = if project.has("rsx-tailwind") || project.has("rsx-htmx") {
            ASSETS_EMBEDDED
        } else {
            ASSETS_NONE
        };
        s.file(
            "src/commands/export.rs",
            EXPORT_RS.replace("@ASSETS@", assets),
        );
        s.at(Slot::CommandsMod).line("pub mod export;");
        s.at(Slot::GitIgnore).line("/dist");
        s.at(Slot::MiseTasks).block(MISE_TASKS);
        s.at(Slot::AgentsSections).block(AGENTS);
    }
}

const MISE_TASKS: &str = r#"
[tasks.export]
description = "write the site to dist/"
run = "cargo run -- export"
"#;

const AGENTS: &str = "\n\
    ## Static site\n\
    \n\
    This project compiles to files. `cargo run -- export` writes every\n\
    route that answers GET to `dist/`, and every asset beside it. `serve`\n\
    is the preview, not the deployment.\n\
    \n\
    - A page must answer the same bytes whatever the request says. It must\n\
    \x20 not read a cookie or the query: the export sends none. The one\n\
    \x20 header it sends is `DP-Partial`, for the partials (`PARTIALS`).\n\
    - A route with a parameter (`users/[id].rs`) is written only for the\n\
    \x20 paths listed in `PATHS` in `src/commands/export.rs`. Add each one.\n\
    - Served under a path (a GitHub Pages project site)? Export with\n\
    \x20 `cargo run -- export --base /<repo>`. Write links from the root\n\
    \x20 (`/about`); the export prefixes them.\n\
    - Do not add a module that runs in a server (a session, a database, a\n\
    \x20 queue, the observability probes). `brocanth add` refuses it.\n";

const EXPORT_RS: &str = r##"//! `export` -- the site, as files.
//!
//! `brocanth-static` does the work. Every route that answers GET is
//! requested once, through the same `handle` that `serve` uses, and its
//! answer is written under `--out`. A page at `/about` becomes
//! `about/index.html`, so `/about` works on any static host. A route whose
//! last segment has a dot (`/feed.xml`) is written under its own name.
//! Every embedded asset is written at the path it is published at, and the
//! 404 page is `404.html`.
//!
//! `--base /my-site` prefixes every root-relative URL in the pages, for a
//! site served under a path, such as a GitHub Pages project site.
//!
//! This file is yours. Add the paths of routes with parameters to `PATHS`.
//! Set `PARTIALS` to the most layout layers a page has to write the
//! partials dp-swap's `partialDir` reads.

use std::path::PathBuf;

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// The concrete paths of routes with a parameter. The export cannot guess
/// which ids exist, so a route like `/users/{id}` is written only for the
/// paths listed here.
const PATHS: &[&str] = &["/users/7"];

/// One partial per layout depth, `1..=PARTIALS`, under `partials/<depth>/`.
/// `0` writes documents only.
const PARTIALS: u32 = 0;

/// Write the site to a directory any static host can serve.
#[cmd]
#[derive(clap::Args)]
pub struct Export {
    /// Directory to write. Emptied first.
    #[arg(long, default_value = "dist")]
    pub out: PathBuf,
    /// The path the site is served under, such as `/my-site`. Empty for
    /// the root of a domain.
    #[arg(long, default_value = "")]
    pub base: String,
}

impl Command for Export {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let app = {
            let _guard = runtime.enter();
            crate::app::build().map_err(anyhow::Error::msg)?
        };

        let site = brocanth_static::Export::new(&self.out)
            .base(&self.base)
            .partials(PARTIALS)
            .routes(&app.router)
            .paths(PATHS.iter().copied())@ASSETS@;
        let report = runtime.block_on(site.run(|request| crate::app::handle(&app, request)))?;

        eprintln!("export: {report} in {}", self.out.display());
        for pattern in &report.skipped {
            eprintln!(
                "export: {pattern} has a parameter: only the paths in PATHS in src/commands/export.rs are written"
            );
        }
        Ok(())
    }
}
"##;

const ASSETS_EMBEDDED: &str = "
            .assets(
                crate::assets::ASSETS
                    .iter()
                    .map(|asset| (asset.published, asset.body)),
            )";

// No embedded assets: nothing in this project writes `assets/` or
// `public/`, and `crate::assets` does not exist.
const ASSETS_NONE: &str = "";
