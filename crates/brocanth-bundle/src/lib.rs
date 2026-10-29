//! JS and CSS bundles, built by `cargo build`.
//!
//! ```ignore
//! // build.rs
//! fn main() {
//!     brocanth_bundle::build().unwrap_or_else(|e| panic!("{e}"));
//! }
//! ```
//!
//! # Inputs are the `_` files of `assets/`
//!
//! `brocanth_assets::assets!` already reads a basename beginning with `_` as
//! an input that is bundled into another and never published. This crate is
//! the other half of that rule:
//!
//! - `assets/<dir>/_name.js` (or `.ts`, `.mjs`) is a JS entry. Rolldown
//!   bundles it, with everything it imports, TypeScript and `node_modules`
//!   included, into `assets/<dir>/name.js`.
//! - `assets/<dir>/_name.css` is a CSS entry. Lightning CSS inlines its
//!   `@import`s into `assets/<dir>/name.css`.
//!
//! A directory whose name begins with `_` (`assets/js/_src/`) holds modules
//! the entries import: neither published nor an entry.
//!
//! The outputs sit in `assets/`, so `assets!` embeds and fingerprints them
//! like any other file, and `assets::url("js/name.js")` names them.
//!
//! # Code that loads later goes to `public/`
//!
//! A dynamic `import()` becomes a chunk of its own, downloaded only when the
//! page runs it. Chunks are written to `public/<dir>/chunks/` with a content
//! hash in the name, because the entry imports them by a relative URL
//! (`./chunks/bar-1a2b3c4d.js`) that a fingerprinted copy would break. The
//! hash already makes the name change when the content does.
//!
//! The one shape this cannot serve is a chunk that imports its entry back:
//! the entry's published name is fingerprinted and the chunk cannot know it.
//! Rolldown does that when a module is both in the entry and in a chunk, so
//! keep an entry small and load the shared code with `import()`. The build
//! fails with the names if it happens.
//!
//! An import of a URL (`import("/reference/x.js")`) stays a URL: the server
//! publishes that file on its own.
//!
//! # `DEV`
//!
//! `DEV=1` or `DEV=true` skips minifying, for code that reads like its
//! source in the browser's debugger. Anything else, or nothing, minifies.
//! Changing it rebuilds: the build script declares the variable.
//!
//! # Rebuilds
//!
//! The build prints `cargo:rerun-if-changed` for every input and every
//! module it bundled, so an edit to any of them rebuilds, and nothing else
//! does. A new entry needs a `touch build.rs`, like a new asset needs a
//! `touch src/assets.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use std::sync::Arc;

use rolldown::{
    Bundler, BundlerOptions, ChunkFilenamesOutputOption, InputItem, IsExternal, OutputFormat,
    Platform, RawMinifyOptions,
};
use rolldown_common::Output;

/// What a build produced, for the build script to report or test.
#[derive(Debug, Default)]
pub struct Report {
    /// Files written, relative to the crate.
    pub written: Vec<PathBuf>,
    /// Every source file read, for `rerun-if-changed`.
    pub sources: Vec<PathBuf>,
}

/// The settings of one build. [`build`] reads them from Cargo's environment.
#[derive(Debug, Clone)]
pub struct Options {
    /// The crate directory: `assets/` and `public/` are under it.
    pub root: PathBuf,
    pub assets: &'static str,
    pub public: &'static str,
    /// Minify JS and CSS.
    pub minify: bool,
}

/// `DEV=1` or `DEV=true`: development, no minifying.
pub fn dev(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "TRUE" | "True"))
}

/// The build-script entry point: bundles `assets/` of the crate being built
/// and tells Cargo what to watch.
pub fn build() -> Result<Report, String> {
    println!("cargo:rerun-if-env-changed=DEV");
    let root = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR")
            .map_err(|_| "CARGO_MANIFEST_DIR is not set; call this from build.rs")?,
    );
    let options = Options {
        root,
        assets: "assets",
        public: "public",
        minify: !dev(std::env::var("DEV").ok().as_deref()),
    };
    let report = bundle(&options)?;
    for source in &report.sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    Ok(report)
}

/// Bundle every `_` entry under `assets/`.
pub fn bundle(options: &Options) -> Result<Report, String> {
    let assets = options.root.join(options.assets);
    let mut report = Report::default();
    // The directory itself: a new file in it re-runs the build.
    report.sources.push(assets.clone());

    let mut groups: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for entry in entries(&assets)? {
        let dir = entry.parent().unwrap_or(&assets).to_path_buf();
        report.sources.push(entry.clone());
        if is_css(&entry) {
            css(&entry, options.minify, &mut report)?;
            continue;
        }
        match groups.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, list)) => list.push(entry),
            None => groups.push((dir, vec![entry])),
        }
    }

    // One rolldown run per directory: entries of one directory share chunks.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("brocanth-bundle: no runtime: {e}"))?;
    for (dir, list) in groups {
        let relative = dir
            .strip_prefix(&assets)
            .unwrap_or(Path::new(""))
            .to_path_buf();
        let chunks = options
            .root
            .join(options.public)
            .join(&relative)
            .join("chunks");
        runtime.block_on(js(&dir, &list, &chunks, options.minify, &mut report))?;
    }
    Ok(report)
}

fn is_css(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "css")
}

/// `_name.js|ts|mjs|css` files, depth first, in name order.
fn entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let Ok(read) = fs::read_dir(dir) else {
        return Ok(out);
    };
    let mut paths: Vec<PathBuf> = read.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if path.is_dir() {
            // `_src/`: modules that entries import, not entries.
            if !name.starts_with('_') {
                out.extend(entries(&path)?);
            }
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if name.starts_with('_') && matches!(ext, "js" | "mjs" | "ts" | "css") {
            out.push(path);
        }
    }
    Ok(out)
}

/// `/x.js`, `//cdn/x.js`, `https://...`: a URL, not a module path.
fn is_url(specifier: &str) -> bool {
    specifier.starts_with('/') && !Path::new(specifier).exists()
        || specifier.starts_with("http://")
        || specifier.starts_with("https://")
}

/// `_name.ts` → `name`.
fn output_name(entry: &Path) -> String {
    let stem = entry
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("bundle");
    stem.trim_start_matches('_').to_string()
}

async fn js(
    dir: &Path,
    list: &[PathBuf],
    chunks: &Path,
    minify: bool,
    report: &mut Report,
) -> Result<(), String> {
    let input = list
        .iter()
        .map(|entry| InputItem {
            name: Some(output_name(entry)),
            import: entry.to_string_lossy().into_owned(),
        })
        .collect();
    let mut bundler = Bundler::new(BundlerOptions {
        input: Some(input),
        cwd: Some(dir.to_path_buf()),
        platform: Some(Platform::Browser),
        format: Some(OutputFormat::Esm),
        entry_filenames: Some(ChunkFilenamesOutputOption::String("[name].js".into())),
        chunk_filenames: Some(ChunkFilenamesOutputOption::String(
            "chunks/[name]-[hash].js".into(),
        )),
        minify: Some(RawMinifyOptions::Bool(minify)),
        // A specifier that is already a URL (`/reference/x.js`, `https://...`) names a file the
        // server publishes on its own: the browser fetches it, the bundle does not include it.
        external: Some(IsExternal::Fn(Some(Arc::new(|specifier, _, _| {
            let url = is_url(specifier);
            Box::pin(async move { Ok(url) })
        })))),
        ..Default::default()
    })
    .map_err(|e| format!("brocanth-bundle: {}: {e:?}", dir.display()))?;
    let output = bundler
        .generate()
        .await
        .map_err(|e| format!("brocanth-bundle: {}: {e:?}", dir.display()))?;
    bundler.close().await.ok();

    // The chunks directory holds only this build's chunks.
    if chunks.exists() {
        fs::remove_dir_all(chunks)
            .map_err(|e| format!("brocanth-bundle: {}: {e}", chunks.display()))?;
    }
    let entries: Vec<String> = output
        .assets
        .iter()
        .filter_map(|o| match o {
            Output::Chunk(c) if c.is_entry => Some(c.filename.to_string()),
            _ => None,
        })
        .collect();
    for item in &output.assets {
        let Output::Chunk(chunk) = item else {
            continue;
        };
        for id in &chunk.module_ids {
            let path = PathBuf::from(id.as_str());
            if path.is_file() && !report.sources.contains(&path) {
                report.sources.push(path);
            }
        }
        let (path, name) = if chunk.is_entry {
            (
                dir.join(chunk.filename.as_str()),
                chunk.filename.to_string(),
            )
        } else {
            // `chunks/x-hash.js` → `public/<dir>/chunks/x-hash.js`.
            let file = chunk.filename.trim_start_matches("chunks/");
            if let Some(entry) = chunk
                .imports
                .iter()
                .find(|i| entries.iter().any(|e| i.ends_with(e.as_str())))
            {
                return Err(format!(
                    "brocanth-bundle: chunk {} imports the entry {entry}, whose published name is fingerprinted. \
                     Load the code the two share with import() from the entry instead.",
                    chunk.filename
                ));
            }
            (chunks.join(file), format!("chunks/{file}"))
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("brocanth-bundle: {}: {e}", parent.display()))?;
        }
        fs::write(&path, chunk.code.as_bytes())
            .map_err(|e| format!("brocanth-bundle: {name}: {e}"))?;
        report.written.push(path);
    }
    Ok(())
}

fn css(entry: &Path, minify: bool, report: &mut Report) -> Result<(), String> {
    use lightningcss::bundler::{Bundler, FileProvider};
    use lightningcss::stylesheet::{ParserOptions, PrinterOptions};

    let fs = FileProvider::new();
    let mut bundler = Bundler::new(&fs, None, ParserOptions::default());
    let sheet = bundler
        .bundle(entry)
        .map_err(|e| format!("brocanth-bundle: {}: {e}", entry.display()))?;
    let css = sheet
        .to_css(PrinterOptions {
            minify,
            ..Default::default()
        })
        .map_err(|e| format!("brocanth-bundle: {}: {e}", entry.display()))?;
    // `@import`ed files, for `rerun-if-changed`: every source the bundle read.
    for source in &sheet.sources {
        let path = PathBuf::from(source);
        if !report.sources.contains(&path) {
            report.sources.push(path);
        }
    }
    let out = entry.with_file_name(format!("{}.css", output_name(entry)));
    fs::write(&out, css.code).map_err(|e| format!("brocanth-bundle: {}: {e}", out.display()))?;
    report.written.push(out);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_is_one_or_true() {
        assert!(dev(Some("1")));
        assert!(dev(Some("true")));
        assert!(!dev(Some("0")));
        assert!(!dev(Some("false")));
        assert!(!dev(None));
    }

    #[test]
    fn bundles_entries_and_splits_dynamic_imports() {
        let root = std::env::temp_dir().join(format!("brocanth-bundle-{}", std::process::id()));
        let js = root.join("assets/js");
        fs::create_dir_all(&js).unwrap();
        fs::write(js.join("_app.ts"), "const n: number = 2;\nexport const go = () => import('./lazy').then((m) => m.twice(n));\n").unwrap();
        fs::write(
            js.join("lazy.ts"),
            "export function twice(x: number): number { return x * 2; }\n",
        )
        .unwrap();
        fs::write(
            root.join("assets/_site.css"),
            "@import './more.css';\na { color: red; }\n",
        )
        .unwrap();
        fs::write(root.join("assets/more.css"), "b { color: blue; }\n").unwrap();

        let report = bundle(&Options {
            root: root.clone(),
            assets: "assets",
            public: "public",
            minify: false,
        })
        .unwrap();
        let app = fs::read_to_string(js.join("app.js")).unwrap();
        assert!(app.contains("./chunks/"), "{app}");
        assert!(!app.contains(": number"), "TypeScript is compiled: {app}");
        let chunk = fs::read_dir(root.join("public/js/chunks"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(fs::read_to_string(chunk).unwrap().contains("x * 2"));
        let css = fs::read_to_string(root.join("assets/site.css")).unwrap();
        assert!(
            css.contains("#00f") && css.contains("red"),
            "the import is inlined: {css}"
        );
        assert!(report.sources.iter().any(|s| s.ends_with("lazy.ts")));
        fs::remove_dir_all(root).ok();
    }
}
