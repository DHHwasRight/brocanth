//! `pages!`: the route table, read off the directory tree.
//!
//! A route is a file, so the set of routes is the set of files. This reads
//! `src/pages` while the crate compiles and emits both halves of what was
//! written by hand before: the `mod` lines, and one `routes.file(..)` row per
//! verb each page answers.

use std::path::{Path, PathBuf};

use brocanth::routing::{generate_helpers, RouteHelper, Verb};
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
    /// What each handler takes and gives, in the order the file declares
    /// them.
    handlers: Vec<Found>,
    /// The route this file is, as a document writes it: `/users/{id}`.
    template: String,
}

/// One handler, read off its signature.
///
/// The name says the verb, the second parameter says what it takes, and
/// the return type says what it gives. Three facts a page already states
/// and nothing read until now.
struct Found {
    /// The function's name.
    name: String,
    /// The `Verb` variant, as an identifier to emit.
    verb_ident: &'static str,
    /// The verb itself.
    verb: Verb,
    /// The second parameter's type, for a handler that takes a body.
    body: Option<syn::Type>,
    /// The `T` of a `Json<T>` return, for a handler that says what it
    /// gives.
    gives: Option<syn::Type>,
}

/// `handle` is GET and `handle_<verb>` is that verb.
///
/// Not a free-form convention: a name outside this list is an error naming
/// the list, because the alternative is a handler that compiles, is never
/// routed, and answers nothing.
const VERBS: [(&str, &str, Verb); 7] = [
    ("handle", "Get", Verb::Get),
    ("handle_head", "Head", Verb::Head),
    ("handle_post", "Post", Verb::Post),
    ("handle_put", "Put", Verb::Put),
    ("handle_patch", "Patch", Verb::Patch),
    ("handle_delete", "Delete", Verb::Delete),
    ("handle_options", "Options", Verb::Options),
];

pub fn expand(root: &Path, manifest: &Path, describe: bool) -> TokenStream2 {
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
        page.handlers.iter().map(move |found| {
            let handler = ident(&found.name);
            let verb = ident(found.verb_ident);
            let path = path.iter();
            // A handler that takes a body is registered through the
            // wrapper that reads one. Both produce the same `Handler`, so
            // the table does not know the difference and the page does
            // not have to say it twice.
            let register = if found.body.is_some() {
                quote!(page_with_body)
            } else {
                quote!(page)
            };
            quote! {
                routes.file(#file, "", Verb::#verb, #register(#(#path ::)* #handler))?;
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

    let helpers = match helpers(root, &pages) {
        Ok(helpers) => helpers,
        Err(message) => return error(&message),
    };

    let described = if describe {
        described(&pages)
    } else {
        TokenStream2::new()
    };

    quote! {
        #mods

        #(#watched)*

        #helpers

        #described

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

/// What every route takes and gives, for the one reader that needs it.
///
/// Emitted only for `pages!("src/pages", describe)`, because the rows name
/// `brocanth_input` and `zod` and a project that chose neither should not
/// be made to depend on them by a macro.
///
/// One table, two readers. An OpenAPI document is this plus the route
/// table; a command-line client is this plus a base URL. Deriving both
/// from the same rows is what stops a document, a client and a handler
/// from disagreeing -- which is the whole claim, and it is checked by the
/// compiler rather than by a review.
fn described(pages: &[Page]) -> TokenStream2 {
    let rows = pages.iter().flat_map(|page| {
        let template = &page.template;
        let module: Vec<_> = page.module.iter().map(|part| ident(part)).collect();
        page.handlers.iter().map(move |found| {
            let verb = ident(found.verb_ident);
            let named = |ty: &Option<syn::Type>| match ty {
                Some(ty) => {
                    let ty = qualified(ty, &module);
                    quote!(::core::option::Option::Some(
                        <#ty as ::zod::Entry>::describe()
                    ))
                }
                None => quote!(::core::option::Option::None),
            };
            let takes = named(&found.body);
            let gives = named(&found.gives);
            quote! {
                ::brocanth_input::Described {
                    path: #template,
                    verb: Verb::#verb,
                    takes: #takes,
                    gives: #gives,
                }
            }
        })
    });

    quote! {
        /// What every route takes and gives, read off the pages while this
        /// crate was compiled.
        ///
        /// A route is a file, a verb is a function, and the signature of
        /// that function says the rest: a second parameter is the body it
        /// takes, and a `Json<T>` return is what it gives. Nothing here is
        /// written by hand, so nothing here can be out of date.
        pub fn described() -> ::std::vec::Vec<::brocanth_input::Described> {
            ::std::vec![#(#rows),*]
        }
    }
}

/// A type as written in a page, said from `src/pages/mod.rs`.
///
/// A page writes `Signup`, and that name means something in the page's own
/// module. The table is emitted one level up, so the name has to carry the
/// module with it: `signup::Signup`.
///
/// A path that already starts somewhere absolute -- `crate`, `::`, `self`,
/// `super` -- is left alone, because it means the same thing from both
/// places. Everything else gets the page's module in front of it, which is
/// the rule the handler itself follows and the reason a body type has to
/// be `pub` in the page that names it.
fn qualified(ty: &syn::Type, module: &[proc_macro2::Ident]) -> TokenStream2 {
    let syn::Type::Path(path) = ty else {
        return quote!(#ty);
    };
    if path.qself.is_some() || path.path.leading_colon.is_some() {
        return quote!(#ty);
    }
    let rooted = path.path.segments.first().is_some_and(|first| {
        first.ident == "crate" || first.ident == "self" || first.ident == "super"
    });
    if rooted {
        return quote!(#ty);
    }
    quote!(#(#module ::)* #ty)
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
    /// The directory this module stands for, where that is not its own
    /// name. `[section]` is not an identifier, so the module is `_section_`
    /// and the directory has to be said out loud.
    dir: Option<String>,
}

impl Node {
    fn of(pages: &[Page]) -> Self {
        let mut root = Node::default();
        for page in pages {
            let mut node = &mut root;
            // The module names and the directory names run in step: the
            // last module is the file, and the ones before it are the
            // directories above it.
            let dirs: Vec<String> = Path::new(&page.route_file)
                .parent()
                .map(|p| {
                    p.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            for (depth, part) in page.module.iter().enumerate() {
                node = node.children.entry(part.clone()).or_default();
                if let Some(dir) = dirs.get(depth) {
                    node.dir = Some(dir.clone());
                }
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
                    // `#[path]` on an inline module is what sets the
                    // directory its own children are looked for in, which
                    // is the only way a module called `_section_` can hold
                    // the files in `[section]/`.
                    let dir = child.dir.clone().unwrap_or_else(|| name.clone());
                    quote! {
                        #[path = #dir]
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

    // The route a document writes, derived by the core from the same file
    // name the table is. One reader, so the two cannot disagree.
    let template = brocanth::routing::route_for_file(&route_file, "")
        .map(|route| route.pattern().to_string())
        .map_err(|e| format!("`{route_file}` is not a route: {e}"))?;

    Ok(Page {
        handlers: handlers(path, &route_file)?,
        route_file,
        source: path.to_path_buf(),
        module,
        template,
    })
}

/// The verbs a page answers, by parsing it.
///
/// Parsed rather than scanned for `pub fn handle`: that text appears in a doc
/// comment in the file the scaffold writes, and a route table that depends on
/// where a comment is would be a route table nobody could trust.
fn handlers(path: &Path, route_file: &str) -> Result<Vec<Found>, String> {
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
        let Some((_, verb_ident, verb)) = VERBS.iter().find(|(candidate, _, _)| *candidate == name)
        else {
            let known = VERBS
                .iter()
                .map(|(name, _, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "`{route_file}` has `pub fn {name}`, which is not a verb: \
                 the framework reads the verb off the name, and the names \
                 it reads are {known}. Rename it, or make it private if it \
                 is not a handler"
            ));
        };
        found.push(Found {
            body: body_of(&function.sig, route_file, &name)?,
            gives: gives_of(&function.sig),
            name,
            verb_ident,
            verb: *verb,
        });
    }
    Ok(found)
}

/// What the handler takes, past the context.
///
/// One parameter is the context and nothing else; two is the context and a
/// body, which the framework reads and checks before the handler runs. A
/// third has nowhere to come from, and saying so here is better than a
/// trait bound nobody can read.
fn body_of(
    sig: &syn::Signature,
    route_file: &str,
    name: &str,
) -> Result<Option<syn::Type>, String> {
    match sig.inputs.len() {
        0 | 1 => Ok(None),
        2 => match &sig.inputs[1] {
            syn::FnArg::Typed(arg) => Ok(Some((*arg.ty).clone())),
            syn::FnArg::Receiver(_) => Err(format!(
                "`{route_file}`: `{name}` takes `self`, and a page is a \
                 function rather than a method"
            )),
        },
        many => Err(format!(
            "`{route_file}`: `{name}` takes {many} parameters. A handler \
             takes the context, and may take one more -- the body, which \
             the framework reads and checks before calling it"
        )),
    }
}

/// What the handler gives, for one that says.
///
/// `Result<Json<T>, Error>` names `T`; anything else names nothing. That
/// is the opt-in, and it is deliberate: `impl IntoResponse` is the
/// documented way to write a page and must keep working, so a page
/// contributes a response schema when it wraps one and not otherwise.
fn gives_of(sig: &syn::Signature) -> Option<syn::Type> {
    let syn::ReturnType::Type(_, returned) = &sig.output else {
        return None;
    };
    inside(returned, "Result").and_then(|ok| inside(ok, "Json").cloned())
}

/// The first type argument of `Name<..>`, as written.
fn inside<'t>(ty: &'t syn::Type, outer: &str) -> Option<&'t syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != outer {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &last.arguments else {
        return None;
    };
    args.args.iter().find_map(|arg| match arg {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    })
}

/// `routes::users::id::path(7)`, one function per route.
///
/// The core already knows how to write these -- `generate_helpers` emits a
/// signature taking exactly that route's parameters, so a wrong count is a
/// compile error rather than a 500. All this does is hand it what `pages!`
/// has already read, instead of making a `build.rs` read the tree a second
/// time.
///
/// The Crystal page_router generates `UserPage.path(123)` from the file
/// location for the same reason: a link is to a route, and a route is a
/// file, so a link should not be a string anybody can mistype.
fn helpers(root: &Path, pages: &[Page]) -> Result<TokenStream2, String> {
    let base = root.to_string_lossy();
    let mut routes = Vec::with_capacity(pages.len());
    for page in pages {
        let file = format!("{base}/{}", page.route_file);
        let helper = RouteHelper::for_file(&file, &base)
            .map_err(|e| format!("`{}` is not a route: {e}", page.route_file))?;
        let verbs: Vec<Verb> = page.handlers.iter().map(|found| found.verb).collect();
        routes.push(helper.with_verbs(verbs));
    }

    let source = generate_helpers(&routes).map_err(|e| e.to_string())?;
    let body: TokenStream2 = source
        .parse()
        .map_err(|e| format!("the generated route helpers do not parse: {e}"))?;

    Ok(quote! {
        /// One function per route: `routes::users::id::path(7)`.
        ///
        /// Generated from the directory, so a link to a route that does not
        /// exist does not compile, and neither does one missing a parameter.
        pub mod routes {
            #body
        }
    })
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
        let out = expand(Path::new("src/pages"), &root, false).to_string();
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
    fn a_link_is_a_function_rather_than_a_string() {
        // Crystal's page_router generates `UserPage.path(123)` from the file
        // location. Same promise: a route is a file, so a link is not a
        // string anybody can mistype.
        let out = table(&[("index.rs", GET), ("users/[id].rs", GET)]);
        assert!(out.contains("pub mod routes"), "{out}");
        // Taking exactly that route's parameters, which is what makes a
        // wrong count a compile error at the call site.
        assert!(out.contains("pub fn path (id : impl"), "{out}");
        assert!(out.contains(r#""/users/""#), "{out}");
    }

    #[test]
    fn the_helpers_know_the_verbs_the_page_answers() {
        let out = table(&[(
            "users/[id].rs",
            "pub fn handle(c: &mut Ctx) -> R { }\npub fn handle_post(c: &mut Ctx) -> R { }",
        )]);
        assert!(out.contains(r#""GET""#), "{out}");
        assert!(out.contains(r#""POST""#), "{out}");
        assert!(!out.contains(r#""DELETE""#), "{out}");
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
