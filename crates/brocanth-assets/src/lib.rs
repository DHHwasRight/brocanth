//! Static files, embedded while the crate compiles.
//!
//! ```ignore
//! // src/assets.rs
//! brocanth_assets::assets!("assets", "public");
//! ```
//!
//! # Two directories, two rules
//!
//! depoth's split, and the reason carries over. **`assets/` is
//! fingerprinted**: `css/site.css` is published at
//! `/css/site.a1b2c3d4.css`, so it can be cached forever and a new build
//! invalidates it by having a different name. **`public/` is served
//! verbatim**: `robots.txt`, `favicon.ico`, and anything a third party links
//! to from outside must keep the name it was given.
//!
//! A fingerprinted URL is only useful if something rewrites the reference to
//! it, which is what [`url`] is for:
//!
//! ```ignore
//! <link rel="stylesheet" href={assets::url("css/site.css")}>
//! ```
//!
//! A name `assets/` does not hold is a compile error at that call, not a 404
//! discovered in a browser.
//!
//! # Embedded rather than read
//!
//! The rest of this framework compiles its templates, its route table and its
//! layouts into the binary, and a stylesheet read from the working directory
//! would be the one thing that could go missing after `cargo build` passed.
//! Embedding also means the fingerprint is computed once, at build time,
//! rather than on every boot.
//!
//! The cost is that a file added where there was none does not rebuild,
//! because nothing references it yet. `touch src/assets.rs`. An edit to a
//! file already there is noticed, because the body is an `include_bytes!`.
//!
//! # A basename beginning with `_` is an input
//!
//! depoth's mark for a file that is bundled into another and must never be
//! published on its own -- `_tailwind.css` is the input of the CSS build,
//! not a stylesheet. Skipped in both directories.

use std::path::{Path, PathBuf};

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use sha2::{Digest, Sha256, Sha384};

/// Hex characters of the SHA-256 kept in a filename.
///
/// Eight is Hugo's and Vite's width: four billion values against a few
/// hundred files.
const FINGERPRINT_LEN: usize = 8;

/// Embeds two directories and emits the lookup, the URL helper and the types.
///
/// `assets!("assets", "public")` -- the first fingerprinted, the second
/// verbatim. Either may be absent, which is not an error: a project with no
/// `public/` has no favicon yet.
#[proc_macro]
pub fn assets(input: TokenStream) -> TokenStream {
    let raw = input.to_string();
    let dirs: Vec<String> = raw
        .split(',')
        .filter_map(|part| string_literal(part.trim()))
        .collect();
    if dirs.len() != 2 {
        return error(
            "expected two string literals, the fingerprinted directory then \
             the verbatim one, as in `brocanth_assets::assets!(\"assets\", \"public\")`",
        )
        .into();
    }

    let root = manifest_dir();
    let mut files = Vec::new();
    for (dir, fingerprint) in [(&dirs[0], true), (&dirs[1], false)] {
        let base = root.join(dir);
        if !base.is_dir() {
            continue;
        }
        if let Err(message) = collect(&base, &base, fingerprint, &mut files) {
            return error(&message).into();
        }
    }
    files.sort_by(|a, b| a.logical.cmp(&b.logical));

    if let Some(clash) = duplicate(&files) {
        return error(&format!(
            "`{clash}` is in both `{}` and `{}`; one logical path cannot have \
             two answers, so rename one or delete it",
            dirs[0], dirs[1]
        ))
        .into();
    }

    expand(&files).into()
}

struct File {
    /// The path an author writes: `css/site.css`.
    logical: String,
    /// The path a browser asks for: `/css/site.a1b2c3d4.css`, or
    /// `/robots.txt` when the file is served verbatim.
    published: String,
    source: PathBuf,
    content_type: &'static str,
    /// `sha384-<base64>`, ready for an `integrity` attribute.
    integrity: String,
    /// The quoted ETag, which is the content hash and therefore strong.
    etag: String,
    /// Whether the published name carries the hash, which is what decides
    /// between caching forever and revalidating.
    fingerprinted: bool,
}

fn collect(base: &Path, dir: &Path, fingerprint: bool, out: &mut Vec<File>) -> Result<(), String> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| format!("cannot read `{}`: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();

    for path in paths {
        if path.is_dir() {
            collect(base, &path, fingerprint, out)?;
            continue;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // depoth's mark for an input: bundled into something else, never
        // published on its own.
        if name.starts_with('_') || name.starts_with('.') {
            continue;
        }
        let bytes =
            std::fs::read(&path).map_err(|e| format!("cannot read `{}`: {e}", path.display()))?;
        let logical = relative(base, &path);
        let digest = Sha256::digest(&bytes);
        let hash: String = hex(&digest)[..FINGERPRINT_LEN].to_string();

        out.push(File {
            published: if fingerprint {
                format!("/{}", stamp(&logical, &hash))
            } else {
                format!("/{logical}")
            },
            content_type: content_type(&path),
            integrity: format!("sha384-{}", base64(&Sha384::digest(&bytes))),
            etag: format!("\"{hash}\""),
            fingerprinted: fingerprint,
            logical,
            source: path,
        });
    }
    Ok(())
}

/// `css/site.css` + `a1b2c3d4` -> `css/site.a1b2c3d4.css`.
///
/// Before the extension rather than after it, so a server, a CDN and a
/// browser all still see a `.css`.
fn stamp(logical: &str, hash: &str) -> String {
    match logical.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}.{hash}.{extension}"),
        None => format!("{logical}.{hash}"),
    }
}

fn relative(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn duplicate(files: &[File]) -> Option<String> {
    files
        .windows(2)
        .find(|pair| pair[0].logical == pair[1].logical)
        .map(|pair| pair[0].logical.clone())
}

fn expand(files: &[File]) -> TokenStream2 {
    let rows = files.iter().map(|file| {
        let logical = &file.logical;
        let published = &file.published;
        let source = file.source.to_string_lossy().into_owned();
        let content_type = file.content_type;
        let integrity = &file.integrity;
        let etag = &file.etag;
        let fingerprinted = file.fingerprinted;
        quote! {
            Asset {
                logical: #logical,
                published: #published,
                body: include_bytes!(#source),
                content_type: #content_type,
                integrity: #integrity,
                etag: #etag,
                fingerprinted: #fingerprinted,
            }
        }
    });
    let count = files.len();

    quote! {
        /// One published file, with everything a response needs about it.
        pub struct Asset {
            /// The path an author writes: `css/site.css`.
            pub logical: &'static str,
            /// The path a browser asks for.
            pub published: &'static str,
            pub body: &'static [u8],
            pub content_type: &'static str,
            /// `sha384-<base64>`, for an `integrity` attribute.
            pub integrity: &'static str,
            /// Strong, because it is the content hash.
            pub etag: &'static str,
            /// Whether the published name carries the hash.
            pub fingerprinted: bool,
        }

        impl Asset {
            /// What to send, and it is one of two answers rather than a
            /// policy. A name carrying a content hash can never be stale, so
            /// it is cached for a year and never revalidated. A name that
            /// does not must be revalidated every time, which is what the
            /// ETag makes cheap.
            pub fn cache_control(&self) -> &'static str {
                if self.fingerprinted {
                    "public, max-age=31536000, immutable"
                } else {
                    "public, max-age=0, must-revalidate"
                }
            }
        }

        /// Every published file, sorted by logical path.
        pub static ASSETS: [Asset; #count] = [#(#rows),*];

        /// The file a browser asked for, by published path.
        pub fn lookup(path: &str) -> Option<&'static Asset> {
            ASSETS.iter().find(|asset| asset.published == path)
        }

        /// The URL to write for a file, by the name its author gave it.
        ///
        /// Panics when nothing is published under that name -- at startup if
        /// it is reached then, which is where a template puts it. A missing
        /// stylesheet is a broken page, and a broken page should not be
        /// something a visitor discovers.
        #[must_use]
        pub fn url(logical: &str) -> &'static str {
            match ASSETS.iter().find(|asset| asset.logical == logical) {
                Some(asset) => asset.published,
                None => panic!(
                    "no asset `{logical}`; `assets!` publishes what is in the \
                     directories it was given, and a file added since the last \
                     build needs `touch src/assets.rs`"
                ),
            }
        }
    }
}

/// By extension, and only the ones a page actually references.
///
/// A table rather than a crate: the list is short, it is in front of the
/// reader, and a guess for an unknown extension is `application/octet-stream`
/// rather than something a browser might execute.
fn content_type(path: &Path) -> &'static str {
    match path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
        .as_str()
    {
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json",
        "map" => "application/json",
        "html" => "text/html; charset=utf-8",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "pdf" => "application/pdf",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Standard base64 with padding, which is what Subresource Integrity wants.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn string_literal(raw: &str) -> Option<String> {
    let inner = raw.trim().strip_prefix('"')?.strip_suffix('"')?;
    (!inner.contains('\\')).then(|| inner.to_string())
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default())
}

fn error(message: &str) -> TokenStream2 {
    quote!(compile_error!(#message);)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_goes_before_the_extension_so_the_type_survives() {
        assert_eq!(stamp("css/site.css", "a1b2c3d4"), "css/site.a1b2c3d4.css");
        assert_eq!(stamp("robots", "a1b2c3d4"), "robots.a1b2c3d4");
    }

    #[test]
    fn base64_matches_the_rfc_including_the_padding() {
        // SRI rejects an unpadded digest, so the padding is not cosmetic.
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn an_unknown_extension_is_not_something_a_browser_will_run() {
        assert_eq!(content_type(Path::new("x.bin")), "application/octet-stream");
        assert_eq!(content_type(Path::new("x")), "application/octet-stream");
        assert_eq!(content_type(Path::new("x.CSS")), "text/css; charset=utf-8");
    }
}
