//! Named stores: a project's second, third, ... object store, beside the
//! default one (`docs/object-storage.md` §9.1).
//!
//! A project has one default store and zero or more named ones, such as
//! private uploads on a disk and public assets in a bucket. Each named
//! store has its own settings (`[storage.<name>]`), its own token key and
//! its own routes. This module holds what the generated code needs and
//! cannot write itself:
//!
//! * [`StoreName`]: the name rule. A name is a TOML table, a flag segment,
//!   a Rust field and a URL segment, so it is one lower case word.
//! * [`Location`] and [`apart`]: two stores at one location fail the start.
//!   They would delete each other's objects and variants.
//! * [`AnyRoutes`] (feature `routes`): the routes of several stores as one
//!   request hook, the named ones first.
//!
//! The token key of a named store comes from
//! [`FileTokenKey::from_secret_for`](crate::FileTokenKey::from_secret_for):
//! the store name is in the domain separator, so a token of one store does
//! not open an address in another.

use std::fmt;
use std::path::{Path, PathBuf};

/// The longest name: a short URL segment and a short flag.
pub const MAX_NAME_LEN: usize = 24;

/// The words a store may not be called.
///
/// `default` names the default store. The others are keys of `[storage]`
/// (each backend's, and the `_file` and `_env` forms of a secret) and of
/// `[storage_routes]`: a name equal to a key would make one TOML key mean a
/// value and a table (D22).
pub const RESERVED: &[&str] = &[
    "default",
    // `[storage]`, every backend.
    "root",
    "migrate",
    "bucket",
    "region",
    "endpoint",
    "path_style",
    "access_key_id",
    "secret_access_key",
    "secret_access_key_file",
    "secret_access_key_env",
    "credentials",
    "credentials_file",
    "credentials_env",
    "prefix",
    "probe",
    "max_object_bytes",
    "secret",
    "secret_file",
    "secret_env",
    "token_ttl",
    // `[storage_routes]`. `path` and `public` are options in
    // `src/modules/storage_routes.rs`, not keys of the section, so a store
    // may be called `public`.
    "max_bytes",
    "dedupe",
    "thumb_sizes",
    "formats",
    "max_pixels",
    "downloads",
    "signed_ttl",
];

/// The Rust keywords a name matches the pattern of: a name is a struct
/// field in the generated code.
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// The name of a named store: `[a-z][a-z0-9_]{0,23}`, not a word of
/// [`RESERVED`] and not a Rust keyword.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StoreName(String);

impl StoreName {
    /// The name, or why it is not one. The reason is one sentence, for a
    /// `help:` line to follow.
    ///
    /// # Errors
    /// The name breaks the pattern, is reserved, or is a Rust keyword.
    pub fn parse(name: &str) -> Result<Self, String> {
        let pattern = name.len() <= MAX_NAME_LEN
            && name.starts_with(|c: char| c.is_ascii_lowercase())
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !pattern {
            return Err(format!(
                "the store name `{name}` is not a lower case letter followed by at most {} lower case letters, digits or `_`",
                MAX_NAME_LEN - 1
            ));
        }
        if RESERVED.contains(&name) {
            return Err(format!(
                "the store name `{name}` is reserved: it is `default` or a key of [storage] or [storage_routes]"
            ));
        }
        if KEYWORDS.contains(&name) {
            return Err(format!(
                "the store name `{name}` is a Rust keyword, and each store is a field of `NamedStores`"
            ));
        }
        Ok(Self(name.to_string()))
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StoreName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a store keeps its objects, for [`apart`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// A directory, as an absolute path.
    Disk(PathBuf),
    /// A folder in a bucket.
    Bucket {
        /// `s3` or `gcs`.
        service: &'static str,
        /// The endpoint, empty for the service's own.
        endpoint: String,
        /// The bucket.
        bucket: String,
        /// The prefix, without a slash at either end.
        prefix: String,
    },
}

impl Location {
    /// A directory. Read after the store opened it: then the directory
    /// exists, and the path is canonical, so a symbolic link or a `..`
    /// cannot make one directory look like two.
    pub fn disk(root: &Path) -> Self {
        let path = std::fs::canonicalize(root).unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|cwd| cwd.join(root))
                .unwrap_or_else(|_| root.to_path_buf())
        });
        Self::Disk(path)
    }

    /// A folder in an S3 bucket.
    pub fn s3(endpoint: Option<&str>, bucket: &str, prefix: Option<&str>) -> Self {
        Self::bucket("s3", endpoint, bucket, prefix)
    }

    /// A folder in a Google Cloud Storage bucket.
    pub fn gcs(endpoint: Option<&str>, bucket: &str, prefix: Option<&str>) -> Self {
        Self::bucket("gcs", endpoint, bucket, prefix)
    }

    fn bucket(
        service: &'static str,
        endpoint: Option<&str>,
        bucket: &str,
        prefix: Option<&str>,
    ) -> Self {
        Self::Bucket {
            service,
            endpoint: endpoint.unwrap_or("").trim_end_matches('/').to_string(),
            bucket: bucket.to_string(),
            prefix: prefix.unwrap_or("").trim_matches('/').to_string(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Location::Disk(path) => format!("the directory {}", path.display()),
            Location::Bucket { bucket, prefix, .. } if prefix.is_empty() => {
                format!("the bucket {bucket:?}")
            }
            Location::Bucket { bucket, prefix, .. } => {
                format!("the folder {prefix:?} of the bucket {bucket:?}")
            }
        }
    }

    fn setting(&self) -> &'static str {
        match self {
            Location::Disk(_) => "root",
            Location::Bucket { .. } => "prefix",
        }
    }
}

/// The section of a store's settings: `[storage]` for the default store,
/// `[storage.<name>]` for a named one.
pub fn section(store: &str) -> String {
    if store == "default" {
        "[storage]".to_string()
    } else {
        format!("[storage.{store}]")
    }
}

/// Refuses two stores at one location: the same directory, or the same
/// endpoint, bucket and prefix (D24). `stores` holds each store's name
/// (`default` for the default store) and location.
///
/// # Errors
/// The first pair that shares a location, as a message with a `help:` line.
pub fn apart(stores: &[(&str, Location)]) -> Result<(), String> {
    for (i, (first, at)) in stores.iter().enumerate() {
        if let Some((second, _)) = stores[i + 1..].iter().find(|(_, other)| other == at) {
            let moved = if *second == "default" { first } else { second };
            return Err(format!(
                "[storage] the stores `{first}` and `{second}` are both at {}, so each would delete the other's objects\n\
                 help: give {} a `{}` of its own",
                at.describe(),
                section(moved),
                at.setting()
            ));
        }
    }
    Ok(())
}

#[cfg(feature = "routes")]
pub use self::routes::{AnyLift, AnyRoutes};

#[cfg(feature = "routes")]
mod routes {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;

    use brocanth::plugin_api::PluginError;
    use brocanth::plugin_hooks::{AsyncRequestHook, Flow, Request, RequestState};

    use crate::routes::{CsrfLift, FileRoutes};

    type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

    /// The routes of several stores as one request hook: each in turn, the
    /// first answer wins. Put the named stores first and the default last.
    ///
    /// The order matters: the default store's routes at `/files` answer
    /// `POST /files/public` with a `405`, because `public` sits where an
    /// address would. The routes of the store at `/files/public` must see
    /// that request first. An address is 64 hex characters, so a store name
    /// never takes one's place.
    #[derive(Clone)]
    pub struct AnyRoutes {
        routes: Arc<[FileRoutes]>,
    }

    impl AnyRoutes {
        /// The routes, in the order they answer.
        pub fn new(routes: Vec<FileRoutes>) -> Self {
            Self {
                routes: routes.into(),
            }
        }

        /// The CSRF lift of every store's routes, as one hook.
        pub fn csrf_lift(&self) -> AnyLift {
            AnyLift {
                lifts: self.routes.iter().map(FileRoutes::csrf_lift).collect(),
            }
        }
    }

    impl AsyncRequestHook for AnyRoutes {
        fn before_dispatch<'a>(
            &'a self,
            request: &'a mut Request,
            state: &'a mut RequestState,
        ) -> BoxFuture<'a, Result<Flow, PluginError>> {
            Box::pin(async move {
                for routes in self.routes.iter() {
                    match routes.before_dispatch(request, state).await? {
                        Flow::Continue => {}
                        answer => return Ok(answer),
                    }
                }
                Ok(Flow::Continue)
            })
        }
    }

    /// The CSRF lifts of several stores. Each one acts only on an upload to
    /// its own path, so all of them run.
    #[derive(Clone)]
    pub struct AnyLift {
        lifts: Arc<[CsrfLift]>,
    }

    impl AsyncRequestHook for AnyLift {
        fn before_dispatch<'a>(
            &'a self,
            request: &'a mut Request,
            _state: &'a mut RequestState,
        ) -> BoxFuture<'a, Result<Flow, PluginError>> {
            for lift in self.lifts.iter() {
                lift.lift(request);
            }
            Box::pin(async { Ok(Flow::Continue) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_lower_case_word() {
        for good in [
            "public",
            "archive",
            "a",
            "assets_2",
            "x23456789012345678901234",
        ] {
            assert_eq!(StoreName::parse(good).expect(good).as_str(), good);
        }
        for bad in [
            "",
            "Public",
            "2nd",
            "_x",
            "a-b",
            "a.b",
            "x234567890123456789012345",
        ] {
            assert!(StoreName::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_name_is_not_a_setting_or_a_keyword() {
        for reserved in [
            "default",
            "bucket",
            "secret",
            "secret_file",
            "max_bytes",
            "downloads",
        ] {
            let said = StoreName::parse(reserved).unwrap_err();
            assert!(said.contains("reserved"), "{said}");
        }
        let said = StoreName::parse("type").unwrap_err();
        assert!(said.contains("keyword"), "{said}");
    }

    #[test]
    fn two_stores_in_one_directory_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let same = dir.path().join(".");
        let said = apart(&[
            ("default", Location::disk(dir.path())),
            ("archive", Location::disk(&same)),
        ])
        .unwrap_err();
        assert!(said.contains("`default` and `archive`"), "{said}");
        assert!(
            said.contains("help: give [storage.archive] a `root`"),
            "{said}"
        );
        assert!(apart(&[
            ("default", Location::disk(dir.path())),
            ("archive", Location::disk(other.path())),
        ])
        .is_ok());
    }

    #[test]
    fn two_stores_in_one_bucket_folder_are_refused() {
        let a = Location::s3(Some("http://minio:9000/"), "assets", Some("/app/"));
        let b = Location::s3(Some("http://minio:9000"), "assets", Some("app"));
        let said = apart(&[("public", a.clone()), ("archive", b)]).unwrap_err();
        assert!(
            said.contains("the folder \"app\" of the bucket \"assets\""),
            "{said}"
        );
        // Another prefix, another service, or another bucket is apart.
        for other in [
            Location::s3(Some("http://minio:9000"), "assets", Some("other")),
            Location::gcs(Some("http://minio:9000"), "assets", Some("app")),
            Location::s3(Some("http://minio:9000"), "uploads", Some("app")),
        ] {
            assert!(apart(&[("public", a.clone()), ("archive", other)]).is_ok());
        }
    }
}
