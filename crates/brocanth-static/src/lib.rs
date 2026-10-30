//! A brocanth application, written to a directory any static host serves.
//!
//! The pages are the same pages. [`Export`] sends each path through the
//! application's own handler, as a GET with no cookie and no query, and
//! writes the answer:
//!
//! - an HTML page at `/a/b` to `a/b/index.html`, so `/a/b` and `/a/b/`
//!   both work on GitHub Pages, S3 or nginx;
//! - a path whose last segment has a dot (`/data/sales.json`) to that name;
//! - for each HTML page, one partial per layout depth `1..=depth` to
//!   `partials/<depth>/a/b/index.html`, requested with `DP-Partial: <depth>`.
//!   dp-swap's `partialDir` reads them (github.com/brocanth/htmx);
//! - whatever a missing path answers to `404.html`;
//! - every asset at the path it is published at.
//!
//! With a base path (`/d3set` for `https://<org>.github.io/d3set/`), every
//! root-relative URL in the HTML and CSS it writes is prefixed. See
//! [`rebase`] for what counts as a URL.
//!
//! # Every depth, always
//!
//! A page with one layout answers depth 2 with the same bytes as depth 1.
//! The export writes both. dp-swap asks for the number of layers the
//! client already holds, which the page it is leaving decides, not the
//! page it is going to: a missing file would be a failed navigation, and
//! a duplicate costs only disk.
//!
//! # Who depends on this
//!
//! The generated `export` command (`brocanth create --with static`) is a
//! thin caller of this crate. The d3set chart gallery
//! (github.com/DHHwasRight/d3set, `rust/gallery/src/export.rs`) calls it
//! directly, with a base path and two layout layers, and publishes the
//! result to GitHub Pages. `tests/d3set.rs` mirrors what that gallery
//! needs, so a change that breaks it fails here first.

use std::collections::HashSet;
use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};

use brocanth::routing::{Router, Verb};
use brocanth_server::{Body, Request, Response};

pub mod rebase;

/// The path the export asks for to learn what a missing page looks like.
pub const MISSING: &str = "/__export_missing__";

/// The header that asks for a partial: how many layout layers to leave out.
pub const PARTIAL_HEADER: &str = "dp-partial";

/// What to write, and where.
#[derive(Debug, Clone)]
pub struct Export {
    out: PathBuf,
    base: String,
    depth: u32,
    paths: Vec<String>,
    skipped: Vec<String>,
    assets: Vec<(&'static str, &'static [u8])>,
}

impl Export {
    /// An export to `out`, with no base path, no partials and no paths.
    pub fn new(out: impl Into<PathBuf>) -> Self {
        Self {
            out: out.into(),
            base: String::new(),
            depth: 0,
            paths: Vec::new(),
            skipped: Vec::new(),
            assets: Vec::new(),
        }
    }

    /// The path the site is served under: `/d3set` for a GitHub Pages
    /// project site. Checked by [`Export::run`].
    #[must_use]
    pub fn base(mut self, base: &str) -> Self {
        self.base = base.to_string();
        self
    }

    /// Writes partials for depths `1..=depth`. `0`, the default, writes
    /// documents only. Set it to the most layout layers a page has.
    #[must_use]
    pub fn partials(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    /// Every route of `router` that answers GET and has no parameter. A
    /// route with a parameter cannot be enumerated: it is listed in
    /// [`Report::skipped`], and its concrete paths go to [`Export::paths`].
    #[must_use]
    pub fn routes<H>(mut self, router: &Router<H>) -> Self {
        for (template, allow) in router.routes() {
            if !allow.iter().any(|verb| verb == Verb::Get) {
                continue;
            }
            if template.params().is_empty() {
                self.paths.push(template.pattern().to_string());
            } else {
                self.skipped.push(template.pattern().to_string());
            }
        }
        self
    }

    /// Concrete paths, such as `/users/7` or `/data/sales.json`. A path
    /// given twice is written once.
    #[must_use]
    pub fn paths<I, S>(mut self, paths: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.paths.extend(paths.into_iter().map(Into::into));
        self
    }

    /// Files written as they are, at their published path. A `.css` file
    /// gets the base path like a page does.
    #[must_use]
    pub fn assets<I>(mut self, assets: I) -> Self
    where
        I: IntoIterator<Item = (&'static str, &'static [u8])>,
    {
        self.assets.extend(assets);
        self
    }

    /// Empties the output directory and writes the site.
    ///
    /// `handle` is the application: the same function `serve` runs for a
    /// request, hooks included.
    ///
    /// # Errors
    ///
    /// The first path that does not answer 200 with a finished body, a
    /// base path that is not a path, an output directory that is not safe
    /// to empty, or a write that fails. Every error says what to do next.
    pub async fn run<F, Fut>(self, handle: F) -> Result<Report, ExportError>
    where
        F: Fn(Request) -> Fut,
        Fut: Future<Output = Response>,
    {
        let base = rebase::normalize(&self.base).map_err(ExportError::Base)?;
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for path in self.paths {
            if !path.starts_with('/') || path.contains(['?', '#']) {
                return Err(ExportError::Path(path));
            }
            if seen.insert(path.clone()) {
                paths.push(path);
            }
        }
        empty(&self.out)?;

        let mut report = Report {
            skipped: self.skipped,
            ..Report::default()
        };
        for path in &paths {
            let (kind, body) = get(&handle, path, None).await?;
            write(&self.out.join(file_for(path)), &kind.rebase(body, &base))?;
            if kind != Kind::Html {
                report.files += 1;
                continue;
            }
            report.pages += 1;
            for depth in 1..=self.depth {
                let (_, body) = get(&handle, path, Some(depth)).await?;
                let at = Path::new("partials")
                    .join(depth.to_string())
                    .join(file_for(path));
                write(&self.out.join(at), &Kind::Html.rebase(body, &base))?;
                report.partials += 1;
            }
        }

        // Whatever a missing path answers, for the hosts that read 404.html.
        let missing = handle(Request::new(Verb::Get, MISSING)).await;
        if missing.status == 404 {
            if let Body::Bytes(bytes) = &missing.body {
                let kind = Kind::of(&missing, MISSING);
                write(
                    &self.out.join("404.html"),
                    &kind.rebase(bytes.to_vec(), &base),
                )?;
                report.not_found = true;
            }
        }

        for (published, body) in self.assets {
            let kind = if published.ends_with(".css") {
                Kind::Css
            } else {
                Kind::Other
            };
            write(
                &self.out.join(published.trim_start_matches('/')),
                &kind.rebase(body.to_vec(), &base),
            )?;
            report.assets += 1;
        }
        Ok(report)
    }
}

/// What the export wrote.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    /// HTML documents.
    pub pages: usize,
    /// Partials, all depths together.
    pub partials: usize,
    /// Paths that answered something other than HTML, such as JSON.
    pub files: usize,
    /// Assets.
    pub assets: usize,
    /// Whether `404.html` was written.
    pub not_found: bool,
    /// Route patterns with a parameter. Each is written only for the
    /// concrete paths given to [`Export::paths`].
    pub skipped: Vec<String>,
}

impl Report {
    /// Every file written.
    pub fn total(&self) -> usize {
        self.pages + self.partials + self.files + self.assets + usize::from(self.not_found)
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} file(s): {} page(s), {} partial(s), {} other file(s), {} asset(s){}",
            self.total(),
            self.pages,
            self.partials,
            self.files,
            self.assets,
            if self.not_found { ", 404.html" } else { "" }
        )
    }
}

/// Why the export stopped.
#[derive(Debug)]
pub enum ExportError {
    /// The base path is not a path prefix. Carries the reason and its help.
    Base(String),
    /// A path that is not absolute, or carries a query or a fragment.
    Path(String),
    /// A path, or one of its partials, did not answer 200.
    Status {
        path: String,
        depth: Option<u32>,
        status: u16,
    },
    /// A path answered a stream, which has no end to write.
    Stream { path: String },
    /// The output directory is not one the export may empty.
    Out(PathBuf),
    /// A file or directory could not be written.
    Io { at: PathBuf, error: std::io::Error },
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Base(why) => write!(f, "base path: {why}"),
            Self::Path(path) => write!(
                f,
                "`{path}` is not a path\n\
                 help: give the path alone, starting with `/`, with no query and no fragment"
            ),
            Self::Status { path, depth: None, status } => write!(
                f,
                "GET {path} answered {status}\n\
                 help: a page in a static site must answer 200 to a bare GET"
            ),
            Self::Status { path, depth: Some(depth), status } => write!(
                f,
                "GET {path} with `DP-Partial: {depth}` answered {status}\n\
                 help: a page must answer every depth up to `partials`; lower `partials` to the most layout layers a page has"
            ),
            Self::Stream { path } => write!(
                f,
                "GET {path} answered a stream\n\
                 help: a static page has an end; return the whole body, or leave the path out"
            ),
            Self::Out(out) => write!(
                f,
                "will not empty `{}`\n\
                 help: export to a directory of its own, such as `dist`",
                out.display()
            ),
            Self::Io { at, error } => write!(
                f,
                "cannot write `{}`: {error}\n\
                 help: check that the output directory is writable",
                at.display()
            ),
        }
    }
}

impl std::error::Error for ExportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// Where a path's answer goes: `about/index.html`, or the name itself when
/// the last segment has a dot.
pub fn file_for(path: &str) -> PathBuf {
    let path = path.trim_matches('/');
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return PathBuf::from(path);
    }
    Path::new(path).join("index.html")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Html,
    Css,
    Other,
}

impl Kind {
    /// By the content type, and by the name when there is none.
    fn of(response: &Response, path: &str) -> Self {
        let kind = response
            .header("content-type")
            .unwrap_or("")
            .to_ascii_lowercase();
        if kind.starts_with("text/html") {
            Self::Html
        } else if kind.starts_with("text/css") || (kind.is_empty() && path.ends_with(".css")) {
            Self::Css
        } else {
            Self::Other
        }
    }

    /// HTML and CSS get the base path; everything else is bytes.
    fn rebase(self, body: Vec<u8>, base: &str) -> Vec<u8> {
        if base.is_empty() || self == Self::Other {
            return body;
        }
        match String::from_utf8(body) {
            Ok(text) if self == Self::Html => rebase::html(&text, base).into_bytes(),
            Ok(text) => rebase::css(&text, base).into_bytes(),
            Err(error) => error.into_bytes(),
        }
    }
}

/// One GET through the application, as a static host would see it.
async fn get<F, Fut>(
    handle: &F,
    path: &str,
    depth: Option<u32>,
) -> Result<(Kind, Vec<u8>), ExportError>
where
    F: Fn(Request) -> Fut,
    Fut: Future<Output = Response>,
{
    let mut request = Request::new(Verb::Get, path);
    if let Some(depth) = depth {
        request = request.with_header(PARTIAL_HEADER, &depth.to_string());
    }
    let response = handle(request).await;
    if response.status != 200 {
        return Err(ExportError::Status {
            path: path.to_string(),
            depth,
            status: response.status,
        });
    }
    let kind = Kind::of(&response, path);
    match response.body {
        Body::Bytes(bytes) => Ok((kind, bytes.to_vec())),
        Body::Stream(_) => Err(ExportError::Stream {
            path: path.to_string(),
        }),
    }
}

/// Removes `out` and creates it again. A path with no last component
/// (`/`, `.`, `..`) is refused: emptying it would delete what the user is
/// working in.
fn empty(out: &Path) -> Result<(), ExportError> {
    if out.file_name().is_none() {
        return Err(ExportError::Out(out.to_path_buf()));
    }
    let io = |error| ExportError::Io {
        at: out.to_path_buf(),
        error,
    };
    if out.exists() {
        std::fs::remove_dir_all(out).map_err(io)?;
    }
    std::fs::create_dir_all(out).map_err(io)
}

fn write(at: &Path, body: &[u8]) -> Result<(), ExportError> {
    let io = |error| ExportError::Io {
        at: at.to_path_buf(),
        error,
    };
    if let Some(dir) = at.parent() {
        std::fs::create_dir_all(dir).map_err(io)?;
    }
    std::fs::write(at, body).map_err(io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_is_a_directory_and_a_file_keeps_its_name() {
        assert_eq!(file_for("/"), PathBuf::from("index.html"));
        assert_eq!(file_for("/about"), PathBuf::from("about/index.html"));
        assert_eq!(file_for("/a/b/"), PathBuf::from("a/b/index.html"));
        assert_eq!(
            file_for("/data/sales.json"),
            PathBuf::from("data/sales.json")
        );
        assert_eq!(file_for("/feed.xml"), PathBuf::from("feed.xml"));
    }

    #[test]
    fn every_error_says_what_to_do() {
        let errors = [
            ExportError::Base(rebase::normalize("/a?b").unwrap_err()),
            ExportError::Path("x".into()),
            ExportError::Status {
                path: "/".into(),
                depth: None,
                status: 500,
            },
            ExportError::Status {
                path: "/".into(),
                depth: Some(2),
                status: 500,
            },
            ExportError::Stream { path: "/".into() },
            ExportError::Out(".".into()),
            ExportError::Io {
                at: "x".into(),
                error: std::io::Error::other("no"),
            },
        ];
        for error in errors {
            assert!(error.to_string().contains("\nhelp: "), "{error}");
        }
    }

    #[test]
    fn the_working_directory_is_never_emptied() {
        for out in [".", "/", ".."] {
            assert!(
                matches!(empty(Path::new(out)), Err(ExportError::Out(_))),
                "{out}"
            );
        }
    }
}
