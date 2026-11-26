//! What `build.rs` calls: check a collection and list its files.
//!
//! ```ignore
//! fn main() {
//!     #[allow(dead_code)]
//!     #[path = "src/plugins/content/posts.rs"]
//!     mod posts;
//!     brocanth_content::build::collection::<posts::Post>("content/posts")
//!         .unwrap_or_else(|e| panic!("{e}"));
//! }
//! ```
//!
//! **Why the build and not the start.** A typo in a post's front matter is
//! a mistake in the source, like a typo in a template. Found by the build,
//! it never reaches a server, and a static export never publishes half a
//! site. Contentlayer and Astro check at build time for the same reason.
//!
//! **Why a list of files and not rendered HTML.** The binary embeds each
//! file with `include_str!` and renders it at start, so the build script
//! links no Markdown or HTML parser and `cargo` rebuilds when a file
//! changes. Rendering a few hundred posts takes milliseconds.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::{is_slug, read_name, FrontMatter};

/// The slugs no entry may take: `tags` is the tag index,
/// `<base>/tags`. The router prefers a literal segment to a parameter, so
/// an entry `tags.md` would never be served.
pub const RESERVED: &[&str] = &["tags"];

/// Checks every `*.md` file in `dir` against `T`, and writes
/// `$OUT_DIR/content_<name>.rs` with the list of them, where `<name>` is the
/// last segment of `dir`.
///
/// `dir` is relative to the package root. Other files in it are ignored,
/// and so are directories.
///
/// The output defines `pub const SOURCES: &[brocanth_content::Source]`, one
/// per file, sorted by path. Include it in the binary:
/// `include!(concat!(env!("OUT_DIR"), "/content_posts.rs"));`.
///
/// # Errors
///
/// Every bad file, one paragraph each: the path, the line, what is wrong,
/// and a `help:` line. A file name that is not a slug, or is the slug
/// [`RESERVED`]; front matter that does not read as `T`; an empty title; a
/// tag that is not a slug; and a translation (`hello.fr.md`) with no file
/// in the default language (`hello.md`).
pub fn collection<T: FrontMatter>(dir: &str) -> Result<(), String> {
    let root = PathBuf::from(
        std::env::var_os("CARGO_MANIFEST_DIR")
            .ok_or("CARGO_MANIFEST_DIR is not set\nhelp: call this from build.rs")?,
    );
    let out = PathBuf::from(
        std::env::var_os("OUT_DIR").ok_or("OUT_DIR is not set\nhelp: call this from build.rs")?,
    );
    // The directory: a file added or removed changes it.
    println!("cargo:rerun-if-changed={dir}");
    let code = check::<T>(&root, dir)?;
    let name = dir.rsplit('/').next().unwrap_or(dir);
    let target = out.join(format!("content_{name}.rs"));
    std::fs::write(&target, code).map_err(|e| {
        format!(
            "cannot write {}: {e}\nhelp: check that OUT_DIR is writable",
            target.display()
        )
    })
}

/// The checks and the generated code, apart from the environment.
pub(crate) fn check<T: FrontMatter>(root: &Path, dir: &str) -> Result<String, String> {
    let full = root.join(dir);
    let listing = std::fs::read_dir(&full).map_err(|e| {
        format!("cannot read {dir}: {e}\nhelp: create the directory, with one `<slug>.md` file per entry")
    })?;
    let mut names: Vec<String> = listing
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.ends_with(".md"))
        .collect();
    names.sort();

    let mut errors: Vec<String> = Vec::new();
    let mut code = format!(
        "// Written by build.rs from {dir}: one entry per file. Do not edit.\n\
         pub const SOURCES: &[brocanth_content::Source] = &[\n"
    );
    let mut defaults: Vec<String> = Vec::new();
    let mut translations: Vec<(String, String)> = Vec::new();
    for name in &names {
        let path = format!("{dir}/{name}");
        // Each file too: an edit inside it does not change the directory.
        println!("cargo:rerun-if-changed={path}");
        let (slug, locale) = match read_name(name) {
            Ok(read) => read,
            Err(e) => {
                errors.push(format!("{path}: {e}"));
                continue;
            }
        };
        if RESERVED.contains(&slug) {
            errors.push(format!(
                "{path}: the slug `{slug}` is the collection's tag index, `<base>/{slug}`\n\
                 help: rename the file: `about-{slug}.md`"
            ));
            continue;
        }
        let file = full.join(name);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) => {
                errors.push(format!("{path}: {e}\nhelp: save the file as UTF-8"));
                continue;
            }
        };
        match brocanth_markdown::front_matter::<T>(&text) {
            Ok(doc) if doc.data.title().trim().is_empty() => errors.push(format!(
                "{path}: the title is empty\nhelp: give the entry a title in its front matter: `title = \"Hello\"`"
            )),
            Ok(doc) => {
                // A tag is the last segment of its page, `<base>/tags/<tag>`.
                for tag in doc.data.tags().iter().filter(|tag| !is_slug(tag)) {
                    errors.push(format!(
                        "{path}: the tag `{tag}` cannot be a URL segment: its page is `<base>/tags/<tag>`\n\
                         help: write it with lower case letters, digits and single `-`: `rust-web`"
                    ));
                }
            }
            Err(e) => errors.push(format!("{path}: {e}")),
        }
        match locale {
            None => defaults.push(slug.to_string()),
            Some(_) => translations.push((slug.to_string(), path.clone())),
        }
        let _ = writeln!(
            code,
            "    brocanth_content::Source {{ path: {path:?}, slug: {slug:?}, locale: {locale:?}, \
             text: include_str!({file:?}) }},",
            file = file.display().to_string(),
        );
    }
    for (slug, path) in translations {
        if !defaults.contains(&slug) {
            errors.push(format!(
                "{path}: a translation with no file in the default language\n\
                 help: add {dir}/{slug}.md, which the list, the feed and every other locale fall back to"
            ));
        }
    }
    if !errors.is_empty() {
        return Err(format!(
            "{} bad file(s) in {dir}:\n\n{}",
            errors.len(),
            errors.join("\n\n")
        ));
    }
    code.push_str("];\n");
    Ok(code)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;
    use crate::Date;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct Post {
        title: String,
        date: Date,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        draft: bool,
    }

    impl FrontMatter for Post {
        fn title(&self) -> &str {
            &self.title
        }
        fn tags(&self) -> &[String] {
            &self.tags
        }
    }

    /// A directory under the target dir, with `files` in `content/posts`.
    fn tree(name: &str, files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("content/posts");
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in files {
            std::fs::write(dir.join(file), text).unwrap();
        }
        root
    }

    #[test]
    fn good_files_become_a_list_the_binary_includes() {
        let root = tree(
            "content-good",
            &[
                ("b.md", "+++\ntitle = \"B\"\ndate = 2026-01-02\n+++\nBody\n"),
                ("a.md", "+++\ntitle = \"A\"\ndate = 2026-01-01\n+++\n"),
                ("a.fr.md", "+++\ntitle = \"A fr\"\ndate = 2026-01-01\n+++\n"),
                ("notes.txt", "not an entry"),
            ],
        );
        let code = check::<Post>(&root, "content/posts").unwrap();
        assert!(
            code.contains("pub const SOURCES: &[brocanth_content::Source]"),
            "{code}"
        );
        let a = code.find("\"content/posts/a.md\"").expect("a.md listed");
        let b = code.find("\"content/posts/b.md\"").expect("b.md listed");
        assert!(a < b, "sorted by path: {code}");
        assert!(code.contains("locale: Some(\"fr\")"), "{code}");
        assert!(!code.contains("notes.txt"), "{code}");
    }

    #[test]
    fn every_bad_file_is_named_with_its_line_and_a_help_line() {
        let root = tree(
            "content-bad",
            &[
                ("missing.md", "+++\ntitle = \"x\"\n+++\n"),
                (
                    "typo.md",
                    "+++\ntitle = \"x\"\ndate = 2026-01-01\ndraf = true\n+++\n",
                ),
                ("empty.md", "+++\ntitle = \" \"\ndate = 2026-01-01\n+++\n"),
                (
                    "Bad_Name.md",
                    "+++\ntitle = \"x\"\ndate = 2026-01-01\n+++\n",
                ),
                (
                    "lonely.fr.md",
                    "+++\ntitle = \"x\"\ndate = 2026-01-01\n+++\n",
                ),
            ],
        );
        let e = check::<Post>(&root, "content/posts").unwrap_err();
        assert!(e.starts_with("5 bad file(s) in content/posts"), "{e}");
        assert!(
            e.contains("content/posts/missing.md: missing field `date`"),
            "{e}"
        );
        assert!(
            e.contains("content/posts/typo.md: line 4: unknown field `draf`"),
            "{e}"
        );
        assert!(
            e.contains("content/posts/empty.md: the title is empty"),
            "{e}"
        );
        assert!(
            e.contains("content/posts/Bad_Name.md: `Bad_Name` cannot"),
            "{e}"
        );
        assert!(
            e.contains("content/posts/lonely.fr.md: a translation"),
            "{e}"
        );
        assert_eq!(e.matches("\nhelp: ").count(), 5, "{e}");
    }

    #[test]
    fn the_tag_index_slug_and_a_tag_that_is_no_url_segment_are_refused() {
        let root = tree(
            "content-tags",
            &[
                ("tags.md", "+++\ntitle = \"x\"\ndate = 2026-01-01\n+++\n"),
                (
                    "spaced.md",
                    "+++\ntitle = \"x\"\ndate = 2026-01-01\ntags = [\"rust\", \"Web Dev\"]\n+++\n",
                ),
                (
                    "fine.md",
                    "+++\ntitle = \"x\"\ndate = 2026-01-01\ntags = [\"rust-web\"]\n+++\n",
                ),
            ],
        );
        let e = check::<Post>(&root, "content/posts").unwrap_err();
        assert!(e.starts_with("2 bad file(s) in content/posts"), "{e}");
        assert!(
            e.contains("content/posts/tags.md: the slug `tags` is the collection's tag index"),
            "{e}"
        );
        assert!(
            e.contains("content/posts/spaced.md: the tag `Web Dev` cannot be a URL segment"),
            "{e}"
        );
        assert!(!e.contains("fine.md"), "{e}");
        assert_eq!(e.matches("\nhelp: ").count(), 2, "{e}");
    }

    #[test]
    fn a_missing_directory_says_to_create_it() {
        let root = std::env::temp_dir().join(format!("content-none-{}", std::process::id()));
        let e = check::<Post>(&root, "content/posts").unwrap_err();
        assert!(e.contains("help: create the directory"), "{e}");
    }
}
