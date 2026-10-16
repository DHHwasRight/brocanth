//! `src/pages`, read while the crate compiles.
//!
//! Two macros, one idea: **a route is a file**, so the directory tree is the
//! source of truth and nothing restates it. `pages!` turns the tree into the
//! route table; `html!` turns a page's position in it into the layouts that
//! wrap the page.
//!
//! ```ignore
//! // src/pages/mod.rs
//! brocanth_pages::pages!("src/pages");   // the mod lines, and router()
//! ```
//!
//! # The rule
//!
//! `layout.rsx` wraps every page in its directory and in every directory
//! below it, and they stack from the root down. A page never names a layout,
//! because which layout covers a page is a property of **where the page is**,
//! not of what it says. That is the rule Crystal's `page_router` uses for
//! `layout.cr`, and this macro is how it is read off the tree in Rust.
//!
//! ```ignore
//! // src/pages/users/[id].rs
//! pub fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
//!     let id: u64 = ctx.require("id")?.parse()?;
//!     html!(views::users::_id_::User { id })
//! }
//! ```
//!
//! With `src/pages/layout.rsx` and `src/pages/users/layout.rsx` present, that
//! expands to the root layout wrapping the `users` layout wrapping the page.
//! Delete `users/layout.rsx` and the same line expands to one layer. Nothing
//! in the page changes either way, and there is no chain to keep in step.
//!
//! # How it knows
//!
//! `Span::call_site().local_file()` gives the path of the file the macro was
//! written in, relative to the package root. Everything else is reading
//! directories: walk up from there to `src/`, and collect a `layout.rsx`
//! wherever one is.
//!
//! Each layout found is also `include_bytes!`d by the expansion, which is
//! what makes cargo rebuild when one is edited. Adding a layout where there
//! was none is the case cargo cannot see on its own -- nothing yet references
//! the file -- so touch any source file in the crate after creating it.
//!
//! # What it assumes
//!
//! Four things, all of them what `brocanth create --with rsx` writes:
//!
//! - pages live under `src/pages`;
//! - `src/pages/mod.rs` has `pub mod views { rsx::load_dir!("src/pages"); }`;
//! - a layout is a file called `layout.rsx`;
//! - a layout component takes `children: Element`, and optionally
//!   `layout: &str`, which `html!` fills with the URL prefix that layout
//!   covers -- `/` at the root, `/users` one directory down;
//!   **no other parameter is filled**;
//! - the context offers `partial_depth(&self) -> u32`: how many layout
//!   layers the client already holds and does not need sent back.
//!
//! The last one is the real constraint. This macro fills `children` and
//! nothing else, because it cannot know what another parameter would mean.
//! A layout that needs a value reads it from a `const` or a function; a value
//! that belongs to one request belongs on the page component, which the
//! handler builds and which already has the context.

mod naming;
mod table;

use std::path::{Path, PathBuf};

use proc_macro::{Span, TokenStream};
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use naming::{ident, to_module_name};

/// The route table, read off the directory tree.
///
/// Takes the pages root, relative to the package: `pages!("src/pages")`. It
/// emits the `mod` lines and a `router()`, which is both halves of what was
/// written by hand before.
///
/// A route is a file, so the set of routes is the set of files. Adding a
/// page is adding a file; renaming one renames its route; deleting one
/// deletes it. There is no list, so there is no list to keep in step.
///
/// The verb comes off the handler's name -- `handle` is GET, `handle_post`
/// is POST -- and the page's source is parsed to find them, because which
/// verbs a page answers is a fact about its Rust and not about its name. A
/// `pub fn handle_<something>` that is not a verb is an error naming the
/// ones that are, rather than a handler that compiles and is never reached.
///
/// Two things it will not do. `mod.rs` is skipped, because it is this table
/// rather than a page; and a page with no handler at all is an error, not an
/// empty row.
#[proc_macro]
pub fn pages(input: TokenStream) -> TokenStream {
    let raw = input.to_string();
    let Some(root) = string_literal(&raw) else {
        return error(
            "expected a string literal with the pages root, as in \
             `brocanth_pages::pages!(\"src/pages\")`",
        );
    };
    table::expand(Path::new(&root), &manifest_dir()).into()
}

/// The path out of `pages!("...")`, without pulling in a parser for it.
fn string_literal(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let inner = trimmed.strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('\\')).then(|| inner.to_string())
}

/// Renders a page inside the layouts that cover it.
///
/// See the crate documentation for the rule and what it assumes.
#[proc_macro]
pub fn html(input: TokenStream) -> TokenStream {
    let Some((ctx, content)) = split_once_comma(TokenStream2::from(input)) else {
        return error(
            "`html!` takes the context and then the page, as in \
             `html!(ctx, views::index::Index { title: TITLE })`; the context \
             is what carries how much of the document the client already has",
        );
    };

    let Some(file) = Span::call_site().local_file() else {
        return error(
            "`html!` cannot tell which file it is in, so it cannot tell which \
             layouts cover this page; call `pages::render` and name the layout \
             yourself",
        );
    };

    let layouts = layouts_above(&file);
    expand(&layouts, &ctx, &content).into()
}

/// The first argument and the rest, split at the first top-level comma.
///
/// By hand rather than with `syn::parse::Parser`: the second half is an
/// arbitrary expression this macro never looks inside, and parsing it would
/// only add ways to reject valid Rust.
fn split_once_comma(input: TokenStream2) -> Option<(TokenStream2, TokenStream2)> {
    let mut first = TokenStream2::new();
    let mut rest = TokenStream2::new();
    let mut seen = false;
    for token in input {
        if !seen {
            if let proc_macro2::TokenTree::Punct(ref punct) = token {
                if punct.as_char() == ',' {
                    seen = true;
                    continue;
                }
            }
            first.extend(std::iter::once(token));
        } else {
            rest.extend(std::iter::once(token));
        }
    }
    if !seen || first.is_empty() || rest.is_empty() {
        return None;
    }
    Some((first, rest))
}

/// The layouts covering `file`, outermost first.
///
/// Walking up rather than down: a page is covered by what is above it, and
/// the directories above a file are a fact about its path rather than
/// something to search for.
fn layouts_above(file: &Path) -> Vec<Layout> {
    let root = manifest_dir();
    let mut found = Vec::new();

    let mut dir = file.parent();
    while let Some(current) = dir {
        // `src` is the ceiling. Above it is the package, which holds
        // `Cargo.toml` and no markup.
        if current.file_name().is_some_and(|name| name == "src") || current.as_os_str().is_empty() {
            break;
        }
        let candidate = root.join(current).join("layout.rsx");
        if candidate.is_file() {
            found.push(Layout {
                module: module_path_from(current),
                prefix: prefix_of(current),
                wants_prefix: declares_prefix(&candidate),
                file: candidate,
            });
        }
        dir = current.parent();
    }

    // Collected leaf to root, applied root to leaf.
    found.reverse();
    found
}

struct Layout {
    /// The directory, as the module path `rsx::load_dir!` gave it. Empty for
    /// the pages root itself.
    module: Vec<String>,
    file: PathBuf,
    /// The URL prefix this layout covers: `/` at the root, `/users` one
    /// directory down. Passed to the component when it declares a `layout`
    /// parameter, which is what dp-swap marks its outlet with.
    prefix: String,
    /// Whether the component declares `layout`. Read off the signature
    /// rather than assumed, so a layout that does not care about partial
    /// navigation does not have to take a parameter it ignores.
    wants_prefix: bool,
}

/// `src/pages` -> `/`, `src/pages/users` -> `/users`.
///
/// The URL prefix a layout covers, which is what dp-swap compares on both
/// sides to find the deepest layer two pages share.
fn prefix_of(dir: &Path) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut seen_src = false;
    for component in dir.components() {
        let name = component.as_os_str().to_string_lossy().into_owned();
        if !seen_src {
            if name == "src" {
                seen_src = true;
            }
            continue;
        }
        parts.push(name);
    }
    // The first part past `src` is the pages directory itself, which is the
    // site root rather than a path segment.
    match parts.len() {
        0 | 1 => "/".to_string(),
        _ => format!("/{}", parts[1..].join("/")),
    }
}

/// Whether a `layout.rsx` declares a `layout` parameter.
///
/// Read off the signature rather than assumed, so a layout with no interest
/// in partial navigation does not have to take a parameter it ignores. A
/// `.rsx` file is not Rust that `syn` will parse -- the body is markup --
/// but the parameter list is, and it is on one line.
fn declares_prefix(file: &Path) -> bool {
    let Ok(source) = std::fs::read_to_string(file) else {
        return false;
    };
    source
        .lines()
        .filter_map(|line| line.split_once("fn Layout("))
        .filter_map(|(_, rest)| rest.split_once(')'))
        .any(|(params, _)| {
            params
                .split(',')
                .any(|param| param.trim().split(':').next().unwrap_or("").trim() == "layout")
        })
}

/// `src/pages/users` -> `["pages", "views", "users"]`.
///
/// The `views` sits where it does because `load_dir!` is written inside a
/// module of that name: the generated `pub mod index` would otherwise collide
/// with the `pub mod index;` naming the page.
fn module_path_from(dir: &Path) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut seen_src = false;
    for component in dir.components() {
        let name = component.as_os_str().to_string_lossy();
        if !seen_src {
            if name == "src" {
                seen_src = true;
            }
            continue;
        }
        parts.push(to_module_name(&name));
        // `views` is declared in the pages root, so it comes directly after
        // it and before any directory below.
        if parts.len() == 1 {
            parts.push("views".to_string());
        }
    }
    parts
}

fn expand(layouts: &[Layout], ctx: &TokenStream2, content: &TokenStream2) -> TokenStream2 {
    // One arm per number of layers the client might already hold. Arm 0 is
    // the whole document, arm 1 skips the root, arm N skips N. This is
    // depoth's partial navigation: depoth prebuilds a tree per depth because
    // it has no server, and this renders the depth that was asked for.
    let arms = (0..=layouts.len()).map(|depth| {
        let mut nested = quote!(#content);
        for layout in layouts[depth..].iter().rev() {
            let path = layout.module.iter().map(|part| ident(part));
            let prefix = &layout.prefix;
            let fields = if layout.wants_prefix {
                quote!(layout: #prefix, children: #nested)
            } else {
                quote!(children: #nested)
            };
            nested = quote! {
                crate:: #(#path ::)* layout::Layout { #fields }
            };
        }
        let pattern = if depth == layouts.len() {
            // Deeper than the chain is the page alone. A client asking for
            // more layers than exist is not an error: it is a client that
            // already holds everything above this page.
            quote!(_)
        } else {
            let depth = depth as u32;
            quote!(#depth)
        };
        quote!(#pattern => crate::pages::render(#nested),)
    });

    // The same trick rsx uses: a file nothing references is a file cargo does
    // not watch, and a layout read at expansion time is referenced by nothing.
    let watched = layouts.iter().map(|layout| {
        let path = layout.file.to_string_lossy().into_owned();
        quote!(
            const _: &[u8] = include_bytes!(#path);
        )
    });

    quote! {
        {
            #(#watched)*
            // How much of the document the client already holds. Zero
            // unless it asked for less, so a first load, a crawler and a
            // reader with JavaScript off all take the first arm.
            match (#ctx).partial_depth() {
                #(#arms)*
            }
        }
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
}

/// A diagnostic in place of the expansion.
///
/// No trailing semicolon: `html!` is used where an expression is wanted, and
/// `compile_error!(..);` there makes rustc report a missing semicolon on top
/// of the real message, which buries it.
fn error(message: &str) -> TokenStream {
    quote!(compile_error!(#message)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout at a directory, with no `layout` parameter. The file does
    /// not exist, which is what `declares_prefix` answers `false` to.
    fn layout(dir: &str) -> Layout {
        Layout {
            module: module_path_from(Path::new(dir)),
            prefix: prefix_of(Path::new(dir)),
            wants_prefix: false,
            file: PathBuf::from("/tmp/layout.rsx"),
        }
    }

    #[test]
    fn the_prefix_is_the_url_the_layout_covers() {
        assert_eq!(prefix_of(Path::new("src/pages")), "/");
        assert_eq!(prefix_of(Path::new("src/pages/users")), "/users");
        assert_eq!(
            prefix_of(Path::new("src/pages/blog/archive")),
            "/blog/archive"
        );
    }

    #[test]
    fn a_layer_the_client_holds_is_a_layer_that_is_not_sent() {
        // Two layouts, so three answers: the document, below the root, and
        // the page alone. depoth prebuilds one tree per depth because it has
        // no server; this picks the arm at request time.
        let layouts = [layout("src/pages"), layout("src/pages/users")];
        let out = expand(&layouts, &quote!(ctx), &quote!(Page {})).to_string();
        assert!(out.contains("0u32 =>"), "{out}");
        assert!(out.contains("1u32 =>"), "{out}");
        assert!(
            out.contains("_ =>"),
            "deeper than the chain is the page: {out}"
        );
        assert!(out.contains("partial_depth ()"), "{out}");
    }

    #[test]
    fn the_prefix_is_passed_only_to_a_layout_that_asked_for_it() {
        let mut marked = layout("src/pages/users");
        marked.wants_prefix = true;
        let out = expand(&[marked], &quote!(ctx), &quote!(Page {})).to_string();
        assert!(out.contains(r#"layout : "/users""#), "{out}");

        let plain =
            expand(&[layout("src/pages/users")], &quote!(ctx), &quote!(Page {})).to_string();
        assert!(!plain.contains("layout : \""), "{plain}");
    }

    #[test]
    fn the_pages_root_reaches_the_views_module_and_stops() {
        assert_eq!(module_path_from(Path::new("src/pages")), ["pages", "views"]);
    }

    #[test]
    fn a_directory_below_it_is_named_under_views() {
        assert_eq!(
            module_path_from(Path::new("src/pages/users")),
            ["pages", "views", "users"]
        );
        assert_eq!(
            module_path_from(Path::new("src/pages/blog/archive")),
            ["pages", "views", "blog", "archive"]
        );
    }

    #[test]
    fn a_directory_whose_name_is_not_an_identifier_gets_the_one_rsx_gave_it() {
        // `[category]` is a route parameter on disk and `_category_` in the
        // module tree. The two have to agree or the layout is not found.
        assert_eq!(
            module_path_from(Path::new("src/pages/blog/[category]")),
            ["pages", "views", "blog", "_category_"]
        );
    }

    #[test]
    fn a_directory_named_for_a_keyword_is_escaped_rather_than_renamed() {
        assert_eq!(
            module_path_from(Path::new("src/pages/match")),
            ["pages", "views", "r#match"]
        );
    }

    #[test]
    fn a_keyword_directory_survives_being_made_into_a_path() {
        // `format_ident!` panics on `r#match`, so this would be a panic
        // inside the compiler rather than an error anyone could read.
        let layouts = [layout("src/pages/match")];
        let out = expand(&layouts, &quote!(ctx), &quote!(Page {})).to_string();
        assert!(out.contains("r#match"), "{out}");
    }

    #[test]
    fn the_layouts_nest_outermost_first() {
        let layouts = [layout("src/pages"), layout("src/pages/users")];
        let out = expand(&layouts, &quote!(ctx), &quote!(Page {})).to_string();
        let root = out.find("views :: layout").expect("the root layout");
        let users = out.find("views :: users :: layout").expect("the users one");
        assert!(root < users, "the root layout must be the outer one: {out}");
        // And each one is watched, or an edit to it changes nothing.
        assert_eq!(out.matches("include_bytes !").count(), 2, "{out}");
    }

    #[test]
    fn no_layout_at_all_is_the_page_and_nothing_else() {
        let out = expand(&[], &quote!(ctx), &quote!(Page {})).to_string();
        assert!(!out.contains("layout"), "{out}");
        assert!(out.contains("crate :: pages :: render"), "{out}");
    }

    #[test]
    fn a_name_starting_with_a_digit_is_still_an_identifier() {
        assert_eq!(
            module_path_from(Path::new("src/pages/2024")),
            ["pages", "views", "_2024"]
        );
    }
}
