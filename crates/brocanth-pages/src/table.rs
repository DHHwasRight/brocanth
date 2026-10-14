//! `pages!`: the route table, read off the directory tree.
//!
//! A route is a file, so the set of routes is the set of files. This reads
//! `src/pages` while the crate compiles and emits both halves of what was
//! written by hand before: the `mod` lines, and one `routes.file(..)` row per
//! verb each page answers.

use std::path::{Path, PathBuf};

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::naming::{ident, to_module_name};

/// A page: one file, and the verbs it answers.
struct Page {
    /// The path relative to the pages root, as `routes.file` wants it:
    /// `users/[id].rs`.
    route_file: String,
    /// The absolute path, for reading and for `include_bytes!`.
    source: PathBuf,
    /// Module names from the pages root down, already made into identifiers.
    module: Vec<String>,
    /// `(handler fn, Verb variant)`, in the order the file declares them.
    handlers: Vec<(String, &'static str)>,
}

/// `handle` is GET and `handle_<verb>` is that verb.
///
/// Not a free-form convention: a name outside this list is an error naming
/// the list, because the alternative is a handler that compiles, is never
/// routed, and answers nothing.
const VERBS: [(&str, &str); 7] = [
    ("handle", "Get"),
    ("handle_head", "Head"),
    ("handle_post", "Post"),
    ("handle_put", "Put"),
    ("handle_patch", "Patch"),
    ("handle_delete", "Delete"),
    ("handle_options", "Options"),
];

pub fn expand(root: &Path, manifest: &Path) -> TokenStream2 {
    let absolute = manifest.join(root);
    let mut pages = Vec::new();
    if let Err(message) = collect(&absolute, &absolute, &mut pages) {
        return error(&message);
    }
    if pages.is_empty() {
        return error(&format!(
            "no pages found under `{}`: a route is a file, so an empty \
             directory is a server that answers nothing",
            root.display()
        ));
    }
    // Sorted, so the generated table does not change order with the
    // filesystem and a diff of it means something.
    pages.sort_by(|a, b| a.route_file.cmp(&b.route_file));

    for page in &pages {
        if page.handlers.is_empty() {
            return error(&format!(
                "`{}` declares no handler, so nothing would route to it; \
                 write `pub fn handle(ctx: &mut Ctx) -> Result<impl \
                 IntoResponse, Error>`, or delete the file",
                page.route_file
            ));
        }
    }

    let mods = modules(&pages);
    let rows = pages.iter().flat_map(|page| {
        let file = &page.route_file;
        let path = page.module.iter().map(|part| ident(part));
        let path: Vec<_> = path.collect();
        page.handlers.iter().map(move |(handler, verb)| {
            let handler = ident(handler);
            let verb = ident(verb);
            let path = path.iter();
            quote! {
                routes.file(#file, "", Verb::#verb, page(#(#path ::)* #handler))?;
            }
        })
    });

    // A page that is added, removed or whose handlers change is a page whose
    // file changed, and this is what makes cargo notice. A brand-new file is
    // the case nothing can reference in advance; that is documented.
    let watched = pages.iter().map(|page| {
        let path = page.source.to_string_lossy().into_owned();
        quote!(
            const _: &[u8] = include_bytes!(#path);
        )
    });

    quote! {
        #mods

        #(#watched)*

        /// The table, read off `src/pages` while this crate was compiled.
        ///
        /// Each row names the file and the framework derives the route from
        /// it. There is no list to keep in step, because there is no list:
        /// adding a file adds a route, and renaming one renames it.
        ///
        /// Registering the same file and verb twice is an error rather than
        /// a silent overwrite -- which cannot happen from here, but can from
        /// a row added by hand beside this one.
        pub fn router() -> Result<Router<Handler>, BuildError> {
            let mut routes = Router::builder();
            #(#rows)*
            routes.build()
        }
    }
}

/// The `mod` lines, nested to match the directories.
///
/// `#[path]` on every one of them rather than only where it is needed: a file
/// called `[id].rs` needs it, and a rule with an exception is a rule someone
/// has to remember.
fn modules(pages: &[Page]) -> TokenStream2 {
    Node::of(pages).render(&[])
}

#[derive(Default)]
struct Node {
    children: std::collections::BTreeMap<String, Node>,
    /// The file name, for the `#[path]` on a leaf.
    file: Option<String>,
}

impl Node {
    fn of(pages: &[Page]) -> Self {
        let mut root = Node::default();
        for page in pages {
            let mut node = &mut root;
            for part in &page.module {
                node = node.children.entry(part.clone()).or_default();
            }
            node.file = Some(
                Path::new(&page.route_file)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        root
    }

    fn render(&self, _at: &[String]) -> TokenStream2 {
        let items = self.children.iter().map(|(name, child)| {
            let module = ident(name);
            match &child.file {
                Some(file) => quote! {
                    #[path = #file]
                    pub mod #module;
                },
                None => {
                    let inner = child.render(&[]);
                    quote! {
                        pub mod #module {
                            #inner
                        }
                    }
                }
            }
        });
        quote!(#(#items)*)
    }
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Page>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            collect(root, &path, out)?;
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        // `mod.rs` is the table itself, which is the one file under here
        // that is not a page.
        if path.file_name().is_some_and(|n| n == "mod.rs") {
            continue;
        }
        out.push(page(root, &path)?);
    }
    Ok(())
}

fn page(root: &Path, path: &Path) -> Result<Page, String> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let route_file = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    let mut module: Vec<String> = relative
        .parent()
        .map(|p| {
            p.components()
                .map(|c| to_module_name(&c.as_os_str().to_string_lossy()))
                .collect()
        })
        .unwrap_or_default();
    module.push(to_module_name(
        &path.file_stem().unwrap_or_default().to_string_lossy(),
    ));

    Ok(Page {
        handlers: handlers(path, &route_file)?,
        route_file,
        source: path.to_path_buf(),
        module,
    })
}

/// The verbs a page answers, by parsing it.
///
/// Parsed rather than scanned for `pub fn handle`: that text appears in a doc
/// comment in the file the scaffold writes, and a route table that depends on
/// where a comment is would be a route table nobody could trust.
fn handlers(path: &Path, route_file: &str) -> Result<Vec<(String, &'static str)>, String> {
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
    let file = syn::parse_file(&source)
        .map_err(|e| format!("`{route_file}` is not valid Rust: {e}; fix it and build again"))?;

    let mut found = Vec::new();
    for item in &file.items {
        let syn::Item::Fn(function) = item else {
            continue;
        };
        if !matches!(function.vis, syn::Visibility::Public(_)) {
            continue;
        }
        let name = function.sig.ident.to_string();
        if !name.starts_with("handle") {
            continue;
        }
        match VERBS.iter().find(|(candidate, _)| *candidate == name) {
            Some((_, verb)) => found.push((name, *verb)),
            None => {
                let known = VERBS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(format!(
                    "`{route_file}` has `pub fn {name}`, which is not a verb: \
                     the framework reads the verb off the name, and the names \
                     it reads are {known}. Rename it, or make it private if it \
                     is not a handler"
                ));
            }
        }
    }
    Ok(found)
}

fn error(message: &str) -> TokenStream2 {
    quote!(compile_error!(#message);)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pages tree on disk, because this macro's whole input is a directory
    /// and a fixture that is not one would be testing something else.
    fn tree(files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "brocanth-pages-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (path, body) in files {
            let full = root.join("src/pages").join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, body).unwrap();
        }
        root
    }

    fn table(files: &[(&str, &str)]) -> String {
        let root = tree(files);
        let out = expand(Path::new("src/pages"), &root).to_string();
        let _ = std::fs::remove_dir_all(&root);
        out
    }

    const GET: &str = "pub fn handle(ctx: &mut Ctx) -> Result<(), Error> { Ok(()) }";

    #[test]
    fn a_file_is_a_route_and_nothing_else_has_to_say_so() {
        let out = table(&[("index.rs", GET), ("about.rs", GET)]);
        assert!(
            out.contains(r#"routes . file ("index.rs" , "" , Verb :: Get"#),
            "{out}"
        );
        assert!(
            out.contains(r#"routes . file ("about.rs" , "" , Verb :: Get"#),
            "{out}"
        );
        assert!(out.contains("pub mod index ;"), "{out}");
        assert!(out.contains("pub mod about ;"), "{out}");
    }

    #[test]
    fn the_verb_is_read_off_the_handlers_name() {
        let out = table(&[(
            "users/[id].rs",
            "pub fn handle(c: &mut Ctx) -> R { }\n\
             pub fn handle_post(c: &mut Ctx) -> R { }\n\
             pub fn handle_delete(c: &mut Ctx) -> R { }",
        )]);
        for verb in ["Verb :: Get", "Verb :: Post", "Verb :: Delete"] {
            assert!(out.contains(verb), "{verb} missing from {out}");
        }
        assert!(!out.contains("Verb :: Put"), "a verb nobody wrote: {out}");
    }

    #[test]
    fn a_file_name_that_is_not_an_identifier_still_reaches_its_module() {
        let out = table(&[("users/[id].rs", GET)]);
        // The route keeps the brackets; the module gets the name rsx would
        // have given the template beside it, so the two agree.
        assert!(out.contains(r#""users/[id].rs""#), "{out}");
        assert!(out.contains(r#"# [path = "[id].rs"]"#), "{out}");
        assert!(out.contains("pub mod users { # [path"), "{out}");
        assert!(out.contains("_id_ :: handle"), "{out}");
    }

    #[test]
    fn a_handler_whose_name_is_not_a_verb_names_the_ones_that_are() {
        // Silence here would be a handler that compiles, is never routed,
        // and answers nothing -- the failure this macro is most able to
        // cause and least able to let stand.
        let out = table(&[("index.rs", "pub fn handle_json(c: &mut Ctx) -> R { }")]);
        assert!(out.contains("compile_error !"), "{out}");
        assert!(out.contains("is not a verb"), "{out}");
        assert!(out.contains("handle_post"), "the message lists them: {out}");
    }

    #[test]
    fn a_private_handler_is_not_a_route_and_is_not_an_error() {
        // `fn handle_thing` without `pub` is a helper, and plenty of pages
        // have one.
        let out = table(&[(
            "index.rs",
            "fn handle_nothing_special() {}\npub fn handle(c: &mut Ctx) -> R { }",
        )]);
        assert!(!out.contains("compile_error !"), "{out}");
        assert!(out.contains("Verb :: Get"), "{out}");
    }

    #[test]
    fn a_page_with_no_handler_at_all_is_an_error() {
        let out = table(&[("index.rs", GET), ("draft.rs", "// later")]);
        assert!(out.contains("declares no handler"), "{out}");
    }

    #[test]
    fn mod_rs_is_the_table_rather_than_a_page() {
        let out = table(&[("index.rs", GET), ("mod.rs", "// the table itself")]);
        assert!(!out.contains(r#""mod.rs""#), "{out}");
        assert!(!out.contains("compile_error !"), "{out}");
    }

    #[test]
    fn a_template_beside_a_page_is_not_a_page() {
        let out = table(&[
            ("index.rs", GET),
            ("index.rsx", "pub fn Index() -> Element { <p>hi</p> }"),
        ]);
        assert!(!out.contains(".rsx"), "{out}");
    }

    #[test]
    fn an_empty_directory_is_a_server_that_answers_nothing() {
        let out = table(&[("layout.rsx", "pub fn Layout() -> Element { <p/> }")]);
        assert!(out.contains("no pages found"), "{out}");
    }

    #[test]
    fn every_page_is_watched_so_an_edit_rebuilds_the_table() {
        let out = table(&[("index.rs", GET), ("about.rs", GET)]);
        assert_eq!(out.matches("include_bytes !").count(), 2, "{out}");
    }

    #[test]
    fn the_rows_are_ordered_by_path_rather_than_by_the_filesystem() {
        // Two runs over the same tree have to produce the same table, or a
        // diff of the expansion says nothing and a build is not reproducible.
        let files = [("users/[id].rs", GET), ("about.rs", GET), ("index.rs", GET)];
        assert_eq!(table(&files), table(&files));
        let out = table(&files);
        let about = out.find(r#""about.rs""#).unwrap();
        let index = out.find(r#""index.rs""#).unwrap();
        let users = out.find(r#""users/[id].rs""#).unwrap();
        assert!(about < index && index < users, "{out}");
    }
}
