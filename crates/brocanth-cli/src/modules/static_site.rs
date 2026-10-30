//! `static`: the page router, compiled to a directory of files.
//!
//! The pages are the same pages. `export` builds the application, asks it
//! for every route that answers GET, and writes each answer to `dist/`,
//! beside every embedded asset. Any static host serves the result: GitHub
//! Pages, an S3 bucket, nginx. `serve` stays, as the preview.
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

use brocanth_scaffold::{Manifest, Module, Project, Scaffold, Slot};

pub struct StaticSite;

impl Module for StaticSite {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "static",
            "`export` writes every GET page and every asset to dist/; no server, so no runtime modules",
        )
        .asked_in("pages", "Compile the pages to files any static host serves")
        .needs("commands", "`export` is a subcommand of this binary")
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
    \x20 not read a cookie, a header or the query: the export sends none.\n\
    - A route with a parameter (`users/[id].rs`) is written only for the\n\
    \x20 paths listed in `PATHS` in `src/commands/export.rs`. Add each one.\n\
    - Do not add a module that runs in a server (a session, a database, a\n\
    \x20 queue, the observability probes). `brocanth add` refuses it.\n";

const EXPORT_RS: &str = r##"//! `export` -- the site, as files.
//!
//! Every route that answers GET is requested once, through the same
//! `handle` that `serve` uses, and its answer is written under `--out`. A
//! page at `/about` becomes `about/index.html`, so `/about` works on any
//! static host. A route whose last segment has a dot (`/feed.xml`) is
//! written under its own name. Every embedded asset is written at the path
//! it is published at, and the 404 page is `404.html`.
//!
//! This file is yours. Add the paths of routes with parameters to `PATHS`.

use std::path::{Path, PathBuf};

use brocanth::routing::Verb;
use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// The concrete paths of routes with a parameter. The export cannot guess
/// which ids exist, so a route like `/users/{id}` is written only for the
/// paths listed here.
const PATHS: &[&str] = &["/users/7"];

/// Write the site to a directory any static host can serve.
#[cmd]
#[derive(clap::Args)]
pub struct Export {
    /// Directory to write. Emptied first.
    #[arg(long, default_value = "dist")]
    pub out: PathBuf,
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

        let mut paths: Vec<String> = Vec::new();
        let mut skipped: Vec<&str> = Vec::new();
        for (template, allow) in app.router.routes() {
            if !allow.iter().any(|verb| verb == Verb::Get) {
                continue;
            }
            if template.params().is_empty() {
                paths.push(template.pattern().to_string());
            } else {
                skipped.push(template.pattern());
            }
        }
        paths.extend(PATHS.iter().map(|path| path.to_string()));

        if self.out.exists() {
            std::fs::remove_dir_all(&self.out)?;
        }
        std::fs::create_dir_all(&self.out)?;

        for path in &paths {
            let (status, body) = runtime.block_on(get(&app, path))?;
            if status != 200 {
                anyhow::bail!(
                    "GET {path} answered {status}\n\
                     help: a page in a static site must answer 200 to a bare GET"
                );
            }
            write(&self.out.join(file_for(path)), &body)?;
        }
        // Whatever a missing path answers, for the hosts that read 404.html.
        let (status, body) = runtime.block_on(get(&app, "/__export_missing__"))?;
        let mut files = paths.len();
        if status == 404 {
            write(&self.out.join("404.html"), &body)?;
            files += 1;
        }
        for asset in assets() {
            write(&self.out.join(asset.0.trim_start_matches('/')), asset.1)?;
            files += 1;
        }

        eprintln!("export: {files} file(s) in {}", self.out.display());
        for pattern in skipped {
            eprintln!(
                "export: skipped {pattern}: it has a parameter; add its paths to PATHS in src/commands/export.rs"
            );
        }
        Ok(())
    }
}

/// One GET, through the hooks, as a static host would see it: no cookie,
/// no header, no query.
async fn get(app: &crate::app::App, path: &str) -> anyhow::Result<(u16, Vec<u8>)> {
    let request = brocanth_server::Request::new(Verb::Get, path);
    let response = crate::app::handle(app, request).await;
    match response.body {
        brocanth_server::Body::Bytes(bytes) => Ok((response.status, bytes.to_vec())),
        brocanth_server::Body::Stream(_) => anyhow::bail!(
            "GET {path} answered a stream\n\
             help: a static page has an end; return the whole body"
        ),
    }
}

/// Where a path's answer goes: `about/index.html`, or the name itself when
/// the last segment has an extension.
fn file_for(path: &str) -> PathBuf {
    let path = path.trim_matches('/');
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return PathBuf::from(path);
    }
    Path::new(path).join("index.html")
}

@ASSETS@

fn write(at: &Path, body: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(at, body)?;
    Ok(())
}
"##;

const ASSETS_EMBEDDED: &str = r#"/// Every embedded asset, at the path it is published at.
fn assets() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    crate::assets::ASSETS
        .iter()
        .map(|asset| (asset.published, asset.body))
}"#;

const ASSETS_NONE: &str = r#"/// No embedded assets: nothing in this project writes `assets/` or
/// `public/`.
fn assets() -> impl Iterator<Item = (&'static str, &'static [u8])> {
    std::iter::empty()
}"#;
