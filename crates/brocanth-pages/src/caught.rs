//! `pages!("src/pages", errors)`: the error pages, read off the tree.
//!
//! Two file names, as in Next.js. `not-found.rsx` answers a 404 for every
//! page in its directory and below it, and for a path under the directory
//! that no route matched. `error.rsx` answers every other failure of a page
//! in its directory and below it. The nearest file wins, and the layouts
//! above it wrap it, as they wrap a page.
//!
//! For a page, "nearest" is a fact about its file, so it is chosen here, at
//! compile time. For a path no route matched there is no file, so the
//! emitted `not_found` compares the path with each directory that holds an
//! error page, deepest first.

use std::path::{Path, PathBuf};

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

use crate::naming::ident;

/// The file that answers a 404, without its extension.
pub const NOT_FOUND: &str = "not-found";
/// The file that answers every other failure, without its extension.
pub const ERROR: &str = "error";
/// The component each file defines.
const NOT_FOUND_COMPONENT: &str = "NotFound";
const ERROR_COMPONENT: &str = "ErrorPage";

/// A directory that holds at least one error page.
struct Dir {
    /// The directory names from the pages root down, as on disk.
    parts: Vec<String>,
    not_found: bool,
    failed: bool,
}

/// The error pages of one tree.
pub struct Caught {
    /// The pages root, relative to the package: `src/pages`.
    root: PathBuf,
    manifest: PathBuf,
    /// Index `i` is the function `not_found_i` or `failed_i`.
    dirs: Vec<Dir>,
}

/// Reads every directory under `root` for `not-found.rsx` and `error.rsx`.
pub fn read(root: &Path, manifest: &Path) -> Result<Caught, String> {
    let mut dirs = Vec::new();
    walk(&manifest.join(root), &mut Vec::new(), &mut dirs)?;
    // The pages root first, then by path, so the emitted names do not
    // change with the order the filesystem lists them.
    dirs.sort_by(|a, b| a.parts.cmp(&b.parts));
    Ok(Caught {
        root: root.to_path_buf(),
        manifest: manifest.to_path_buf(),
        dirs,
    })
}

fn walk(dir: &Path, parts: &mut Vec<String>, out: &mut Vec<Dir>) -> Result<(), String> {
    let has = |stem: &str| dir.join(format!("{stem}.rsx")).is_file();
    let (not_found, failed) = (has(NOT_FOUND), has(ERROR));
    if not_found || failed {
        out.push(Dir {
            parts: parts.clone(),
            not_found,
            failed,
        });
    }
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
    let mut children: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.is_dir())
        .collect();
    children.sort();
    for child in children {
        let name = child
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        parts.push(name);
        walk(&child, parts, out)?;
        parts.pop();
    }
    Ok(())
}

impl Caught {
    /// The two error pages nearest a page file, `users/[id].rs`, as the
    /// arguments of `catching`: `Some(__error_pages::not_found_0)` or
    /// `None`.
    pub fn nearest_to_file(&self, route_file: &str) -> (TokenStream2, TokenStream2) {
        let mut parts: Vec<String> = route_file.split('/').map(str::to_string).collect();
        parts.pop();
        self.nearest(&parts)
    }

    /// The nearest `not-found.rsx` and `error.rsx` at or above `parts`.
    fn nearest(&self, parts: &[String]) -> (TokenStream2, TokenStream2) {
        let find = |wanted: fn(&Dir) -> bool, prefix: &str| {
            (0..=parts.len())
                .rev()
                .find_map(|depth| {
                    self.dirs
                        .iter()
                        .position(|dir| dir.parts == parts[..depth] && wanted(dir))
                })
                .map_or(quote!(None), |index| {
                    let name = format_ident!("{prefix}_{index}");
                    quote!(Some(__error_pages::#name))
                })
        };
        (
            find(|dir| dir.not_found, "not_found"),
            find(|dir| dir.failed, "failed"),
        )
    }

    /// The emitted items: one function per error page, and `not_found`.
    pub fn items(&self) -> TokenStream2 {
        let pages = self.dirs.iter().enumerate().flat_map(|(index, dir)| {
            let mut out = Vec::new();
            if dir.not_found {
                out.push(self.page(index, dir, NOT_FOUND, NOT_FOUND_COMPONENT, "not_found"));
            }
            if dir.failed {
                out.push(self.page(index, dir, ERROR, ERROR_COMPONENT, "failed"));
            }
            out
        });

        // Deepest first, so `/users/7/x` finds `users/[id]/not-found.rsx`
        // before `users/not-found.rsx`. The pages root is the last answer
        // and needs no test.
        let mut deeper: Vec<&Dir> = self.dirs.iter().filter(|d| !d.parts.is_empty()).collect();
        deeper.sort_by(|a, b| {
            b.parts
                .len()
                .cmp(&a.parts.len())
                .then(a.parts.cmp(&b.parts))
        });
        let (root_nf, root_er) = self.nearest(&[]);
        let choice = if deeper.is_empty() {
            quote!((#root_nf, #root_er))
        } else {
            let arms = deeper.iter().map(|dir| {
                let pattern = dir.parts.iter().map(|part| {
                    if part.starts_with('[') && part.ends_with(']') {
                        quote!(None)
                    } else {
                        quote!(Some(#part))
                    }
                });
                let (nf, er) = self.nearest(&dir.parts);
                quote!(if __under(&segments, &[#(#pattern),*]) { (#nf, #er) } else)
            });
            quote! {{
                let segments: Vec<&str> =
                    ctx.path.split('/').filter(|s| !s.is_empty()).collect();
                #(#arms)* { (#root_nf, #root_er) }
            }}
        };
        let under = (!deeper.is_empty()).then(|| {
            quote! {
                /// Whether a path lies under a directory of `src/pages`. `None`
                /// is a directory named after a parameter, which any segment
                /// fills.
                fn __under(path: &[&str], prefix: &[Option<&str>]) -> bool {
                    path.len() >= prefix.len()
                        && prefix
                            .iter()
                            .zip(path)
                            .all(|(want, got)| want.is_none_or(|w| w == *got))
                }
            }
        });

        quote! {
            /// The error pages of `src/pages`, one function per file, each
            /// wrapped in the layouts above it. `catching` and `not_found`
            /// name them.
            #[doc(hidden)]
            pub mod __error_pages {
                #(#pages)*
            }

            #under

            /// The answer for a path no route matched: the nearest
            /// `not-found.rsx` above the path, inside its layouts.
            /// `src/app.rs` calls it.
            pub fn not_found(ctx: &mut crate::pages::Ctx) -> crate::pages::Response {
                let pages: (
                    Option<crate::pages::ErrorPage>,
                    Option<crate::pages::ErrorPage>,
                ) = #choice;
                crate::pages::caught(
                    ctx,
                    crate::pages::Error::new(404, "Not Found"),
                    pages.0,
                    pages.1,
                )
                .unwrap_or_else(crate::pages::IntoResponse::into_response)
            }
        }
    }

    /// One error page as a function: the component inside the layouts
    /// above its file.
    fn page(
        &self,
        index: usize,
        dir: &Dir,
        stem: &str,
        component: &str,
        prefix: &str,
    ) -> TokenStream2 {
        let directory = dir
            .parts
            .iter()
            .fold(self.root.clone(), |path, part| path.join(part));
        let file = directory.join(format!("{stem}.rsx"));
        let layouts = match crate::layouts_above_in(&self.manifest, &file) {
            Ok(layouts) => layouts,
            Err(why) => return quote!(compile_error!(#why);),
        };
        let module = crate::module_path_from(&directory);
        let path = module.iter().map(|part| ident(part));
        let file_module = ident(&crate::naming::to_module_name(stem));
        let component = format_ident!("{component}");
        let fields =
            if crate::declares(&self.manifest.join(&file), &component.to_string(), "caught") {
                quote!({ caught })
            } else {
                quote!({})
            };
        let content = quote!(crate:: #(#path ::)* #file_module :: #component #fields);
        let body = crate::expand(&layouts, &quote!(ctx), &content);
        let name = format_ident!("{prefix}_{index}");
        quote! {
            #[allow(unused_variables)]
            pub fn #name(
                ctx: &mut crate::pages::Ctx,
                caught: &crate::pages::Caught,
            ) -> Result<crate::pages::Response, crate::pages::Error> {
                #body
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "brocanth-caught-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for path in files {
            let full = root.join("src/pages").join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            let body = if path.ends_with("not-found.rsx") {
                "pub fn NotFound(caught: &Caught) -> Element { <p/> }"
            } else if path.ends_with("error.rsx") {
                "pub fn ErrorPage() -> Element { <p/> }"
            } else {
                "pub fn Layout(children: Element) -> Element { {children} }"
            };
            std::fs::write(full, body).unwrap();
        }
        root
    }

    #[test]
    fn a_page_takes_the_nearest_file_of_each_kind() {
        let root = tree(&["not-found.rsx", "error.rsx", "users/not-found.rsx"]);
        let caught = read(Path::new("src/pages"), &root).expect("read");
        let (nf, er) = caught.nearest_to_file("users/[id].rs");
        // `users/` has its own not-found page, and no error page, so the
        // root's error page answers.
        assert_eq!(nf.to_string(), "Some (__error_pages :: not_found_1)");
        assert_eq!(er.to_string(), "Some (__error_pages :: failed_0)");
        let (nf, _) = caught.nearest_to_file("about.rs");
        assert_eq!(nf.to_string(), "Some (__error_pages :: not_found_0)");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tree_with_no_error_page_names_none() {
        let root = tree(&["layout.rsx"]);
        let caught = read(Path::new("src/pages"), &root).expect("read");
        let (nf, er) = caught.nearest_to_file("index.rs");
        assert_eq!(
            (nf.to_string(), er.to_string()),
            ("None".into(), "None".into())
        );
        let out = caught.items().to_string();
        assert!(!out.contains("__under"), "no directory to test: {out}");
        assert!(out.contains("pub fn not_found"), "{out}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_error_page_is_wrapped_in_the_layouts_above_it() {
        let root = tree(&["layout.rsx", "users/[id]/not-found.rsx", "error.rsx"]);
        let caught = read(Path::new("src/pages"), &root).expect("read");
        let out = caught.items().to_string();
        assert!(
            out.contains("pages :: views :: users :: _id_ :: not_found :: NotFound { caught }"),
            "the component, with the value it declares: {out}"
        );
        assert!(
            out.contains("pages :: views :: error :: ErrorPage { }"),
            "nothing filled that it does not declare: {out}"
        );
        assert!(out.contains("views :: layout :: Layout"), "{out}");
        // A parameter directory is a hole any segment fills.
        assert!(out.contains("& [Some (\"users\") , None]"), "{out}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
