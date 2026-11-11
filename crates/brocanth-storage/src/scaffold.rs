//! `storage` -- what `brocanth create` writes when somebody asks to keep
//! files.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: `LocalDisk::open`, that
//! [`with_hash_key`](crate::LocalDisk::with_hash_key) is the difference
//! between an address anybody can compute and one nobody can, that
//! [`FileTokenKey`](crate::FileTokenKey) and [`HashKey`](crate::HashKey)
//! derive separately from one secret, and that opening a store is
//! synchronous. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! # What it does not write
//!
//! **An upload route or a download route.** Those are the child
//! [`StorageRoutes`], `storage-routes`, which a project asks for. Minting a
//! file token stays a decision about *this* application's policy, and so
//! does who may upload: the child writes an `UploadGuard` that answers no
//! until the project edits it, and no route mints a token on its own. The
//! plugin puts the store and the token service on the context and on the
//! services, and the generated file's own header says what the two calls
//! are.
//!
//! **Image variants.** `variants` pulls four image decoders in, and §8.2 asks
//! that a feature which is off means the code is absent. A project that
//! stores PDFs must not link a PNG decoder because a menu ticked it by
//! default, so the dependency goes in without the feature and the generated
//! file says in one paragraph what turning it on costs and buys.
//!
//! It is also not a sub-choice on the menu, and that is a fact about the CLI
//! rather than a preference: `create` renders one dependency line per `Dep`
//! and merges nothing, so a second module declaring `brocanth-storage` with
//! `features = ["variants"]` would write the crate into `[dependencies]`
//! twice and produce a manifest cargo refuses to read.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{
    configurable, configurable_hooking, module_ref, settings_derive, Hooking, PLUGIN_TRAIT_USE,
};
use brocanth_scaffold::{
    Category, Dep, Hook, Manifest, Module, Opt, Project, Scaffold, Setting, Slot,
};

/// File storage, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct FileStorage;

impl Module for FileStorage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage",
            "uploads on a local disk, addressed by content hash; no object store and no S3",
        )
        .category(Category::Enhancer)
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .asked_in("data", "Uploads on disk, addressed by content hash")
        .krate(Dep::brocanth("brocanth-storage"))
        // Only for the order of the `[storage]` section and the builder line:
        // the store reads its root and its secret from the file the config
        // plugin loaded, so it belongs under it rather than above it.
        .after("config")
        .absent(
            "brocanth-storage",
            "no file storage; an upload has nowhere to go and no address to be read back from",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/storage/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod storage;");
        // What makes the store a thing the project reads rather than a
        // field nothing touches. A generated application that warns on its
        // first build is a bad first minute, and §4.8 says a scaffold that
        // produces code you would not write by hand is a scaffold whose
        // framework is wrong.
        s.at(Slot::AppContext).block(
            "    let files: &plugins::storage::FileStore = cx.extract();\n\
            \x20   let ttl = files.tokens.ttl();\n\
            \x20   crate::plugins::say!(info, \"storage: ready, download links last {ttl}s\"; ttl);"
                .to_string(),
        );
        configurable(s, project, "storage", "Files", settings());
        // The one module so far that writes files into the working tree, so
        // the one that has to say they are not source. The path matches the
        // `root` below; change one and change the other.
        s.at(Slot::GitIgnore).line("/var/uploads");
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait method,
            // so the trait has to be in scope. Every plugin module needs the
            // same line, and `use` twice does not compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// The `[storage]` settings: the root, the one secret, and the token's life.
///
/// The secret has no flag. A flag is in `ps` output for every user on the
/// host, and this one mints download links for every object.
fn settings() -> Vec<Setting> {
    vec![
        Setting::text("root", "var/uploads", "Where uploads are stored.").note(
            "Below the working directory, so an upload is not source and `.gitignore`\n\
             already says so. Move it off the deployment's disk and every instance stops\n\
             seeing the same files: an object store is a separate crate behind the same\n\
             trait, and swapping to it is a change in src/plugins/storage/mod.rs alone.",
        ),
        Setting::text(
            "secret",
            "development only: replace before anything is uploaded in anger",
            "Signs file tokens and keys object addresses.",
        )
        .note(
            "Development only, and the one line to change first. While it is this\n\
             string, anybody who has read this repository can mint a token for any\n\
             object, and anybody holding a copy of a file can compute whether somebody\n\
             uploaded it. No flag sets it: a flag is readable by every user on the host.",
        )
        .secret(),
        Setting::integer("token_ttl", 120, "Seconds a file token lives.").note(
            "It cannot be revoked, so this number is the window in which a URL pasted\n\
             into a chat still works. Two minutes is long enough for a page full of\n\
             images and short enough to be dead before anyone clicks it.",
        ),
    ]
}

fn plugin_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "storage");
    format!(
        r##"//! File storage, as this application registers it.
//!
//! An object is named by what is in it. Thirty-two bytes rendered as
//! sixty-four hex characters is the whole address, so `..`, a slash and a
//! drive letter are not values the type can hold and no filename a client
//! sent ever becomes a path component. The original name is kept as metadata,
//! defanged for the `Content-Disposition` header it ends up in, and never
//! touches the filesystem.
//!
//! `brocanth-storage` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/plugins/mod.rs`
//! and delete this file, and the application stores no files and links
//! nothing to store them with.
//!
//! # Uploading
//!
//! There is no upload route here, on purpose: `brocanth-storage` parses no
//! multipart body and knows nothing about HTTP, and whether *this*
//! application lets *this* request store a file is a policy question a
//! generator cannot answer. A handler that has already asked it writes:
//!
//! ```ignore
//! let files = ctx.provided::<FileStore, _>().expect("a dispatched request");
//! let about = ObjectInfo::new()
//!     .with_filename(sent_name)
//!     .with_declared_type(sent_type);
//! let key = files.store.put(&bytes, &about).await?;
//! ```
//!
//! and keeps `key.to_hex()` on the record. The media type is taken from the
//! bytes rather than from `sent_type`, because a client that labels a file
//! `text/html` is choosing what this origin serves back to the next visitor.
//!
//! # Serving
//!
//! Also the application's, and for a sharper reason: a file token says "the
//! bearer may read this object" and carries no identity at all. Mint one only
//! for a request that has already passed the policy, because after that the
//! token is the only thing between the object and whoever the URL reaches.
//!
//! ```ignore
//! let token = files.tokens.issue(&key);           // after the policy said yes
//! let key = files.tokens.verify(presented)?;      // on the download route
//! let meta = files.store.meta(&key).await?;
//! ```
//!
//! `ObjectMeta::content_type` answers `is_safe_to_inline`, which is the bit
//! the download route needs to force an attachment for everything that is not
//! a recognised image.
//!
//! # Thumbnails
//!
//! `get_variant` and `put_variant` are here already, with the `variants`
//! feature off: moving derived bytes needs no codec, so this build can serve
//! a variant another process computed and cannot compute one itself. Turning
//! the feature on in `Cargo.toml` adds `derive_variant`, and adds png, jpeg,
//! gif and webp decoders pointed at stranger-supplied bytes. An application
//! that stores documents should leave it off.
//!
//! # One secret, two keys
//!
//! `secret` below is used twice and the two uses cannot be confused for each
//! other: `HashKey` and `FileTokenKey` derive from it under different domain
//! separators, so an address key cannot sign a token and a token key cannot
//! address an object. Rotating the secret invalidates every outstanding token
//! at once, which is the break-glass, and it also changes the address new
//! uploads are stored at -- objects already on disk keep the address they
//! were written under, so rotate it with a plan rather than on a Friday.

use std::sync::Arc;

use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_storage::{{FileTokenKey, FileTokens, HashKey, LocalDisk, Storage}};

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// Where the objects live, relative to the working directory or absolute.
    /// Created if it is not there.
    pub root: String,
    /// The deployment secret. It signs file tokens and keys object addresses;
    /// see the module header for what each of those means.
    pub secret: String,
    /// How many seconds a file token lives. A token cannot be revoked, so
    /// this is the leak window.
    pub token_ttl: i64,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            root: "var/uploads".to_string(),
            secret: "development only: replace before anything is uploaded in anger".to_string(),
            token_ttl: 120,
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// A wrapper and not the two values loose, because `ContextItem` is
/// brocanth's trait and both of these are `brocanth-storage`'s types. That
/// crate deliberately does not depend on the core -- it has no routes, parses
/// no multipart body and knows nothing about HTTP -- so the impl that joins
/// the two belongs in the one crate that can see both, which is this one.
pub struct FileStore {{
    /// The store, behind the trait rather than as a `LocalDisk`, so swapping
    /// the backend is a change in this file and nowhere else.
    ///
    /// Allowed to be unread, and only until one thing changes. Every
    /// `Storage` method is `async` and the startup line below is not, so
    /// nothing the generator writes can touch this field — a handler is
    /// what uses it, and a handler cannot reach the per-request context
    /// yet, because `Ctx` in `src/pages/mod.rs` has no extractor. Delete
    /// this attribute the moment you write the first upload route; it is
    /// a note about today, not about the field.
    #[allow(dead_code)]
    pub store: Arc<dyn Storage>,
    /// Mints and verifies the short-lived tokens a download route checks.
    pub tokens: FileTokens,
}}

impl ContextItem for FileStore {{}}

pub struct Files {{
    settings: Settings,
    /// Built at `configure`, because opening a store can fail and
    /// `Plugin::new` cannot.
    store: Option<Arc<dyn Storage>>,
    tokens: Option<FileTokens>,
}}

impl Plugin for Files {{
    type Config = Settings;
    type Provides = ProvidesContext<FileStore>;
    const NAME: &'static str = "storage";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            store: None,
            tokens: None,
        }}
    }}

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        if self.settings.root.is_empty() {{
            return Err(PluginError::new(
                "[storage] root is required; there is no default directory",
            ));
        }}
        if self.settings.secret.is_empty() {{
            // An empty secret is not a weaker store, it is two different
            // stores: addresses stop being keyed, and a token key of no bytes
            // signs tokens anybody can forge.
            return Err(PluginError::new(
                "[storage] secret is required; it signs file tokens and keys object addresses",
            ));
        }}
        if self.settings.token_ttl <= 0 {{
            return Err(PluginError::new(
                "[storage] token_ttl must be at least one second, or no token is ever valid",
            ));
        }}
        // Synchronous: `std::fs`, a `create_dir_all` and a `canonicalize`.
        // Unlike a pool there is nothing to open, so there is no
        // `before_start`.
        let disk = LocalDisk::open(&self.settings.root)
            .map_err(|e| {{
                PluginError::new(format!("[storage] opening {{:?}}: {{e}}", self.settings.root))
            }})?
            .with_hash_key(HashKey::from_secret(self.settings.secret.as_bytes()));
        let store: Arc<dyn Storage> = Arc::new(disk);
        let tokens = FileTokens::new(FileTokenKey::from_secret(self.settings.secret.as_bytes()))
            .with_ttl(self.settings.token_ttl);
        // On the services too, where a plugin registered after this one
        // reads them at its own `configure`: the upload and download routes
        // of `storage-routes` do.
        lc.services_mut().insert(Arc::clone(&store));
        lc.services_mut().insert(tokens.clone());
        self.store = Some(store);
        self.tokens = Some(tokens);
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // both are here by the time any request builds a context.
        if let (Some(store), Some(tokens)) = (&self.store, &self.tokens) {{
            cx.insert(FileStore {{
                store: store.clone(),
                tokens: tokens.clone(),
            }});
        }}
    }}
}}
"##
    )
}

/// The HTTP edge of the store: `storage-routes`, a child of `storage`.
///
/// It writes `src/plugins/storage_routes/mod.rs`: the settings, the
/// project's `may_upload` guard, and a plugin that installs the crate's
/// [`FileRoutes`](crate::routes::FileRoutes) as two request hooks around the
/// CSRF check. Off when `storage` is chosen: a project that stores files
/// from a job or a handler needs no route.
pub struct StorageRoutes;

impl Module for StorageRoutes {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-routes",
            "POST /files and GET /files/{address}?token=: uploads your guard allows, downloads behind a file token, thumbnails",
        )
        .category(Category::Middleware)
        .part_of_but_off("storage")
        .because("it is the HTTP edge of `brocanth-storage`, which the parent brings")
        .asked_in("data", "Upload and download routes, file tokens and thumbnails")
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        // `routes` is the HTTP edge; `variants` renders a thumbnail the
        // first time a size is asked for. Image decoders are what
        // `?thumb=` costs, and this module is asked for, never ticked by
        // default.
        .krate(
            Dep::brocanth("brocanth-storage")
                .feature("routes")
                .feature("variants"),
        )
        // The plugin reads the store and the token service from the
        // services the storage plugin fills, at its own `configure`.
        .after("storage")
        .after("config")
        .hook(
            "answer",
            "answer POST {path} and GET {path}/{address}",
            Hook::required(),
        )
        .hook(
            "lift",
            "copy an upload form's CSRF field into the header the CSRF check reads",
            Hook::on(),
        )
        .option(Opt::text(
            "path",
            "/files",
            "Where the routes answer: uploads at {path}, files at {path}/{address}.",
        ))
        .option(Opt::bool(
            "public",
            false,
            "Serve a file to a request with no token. Off, every download needs a token.",
        ))
        .option(Opt::bool(
            "thumbnails",
            true,
            "Answer ?thumb=WxH with a thumbnail, rendered once per size.",
        ))
        .absent(
            "storage-routes",
            "no upload or download route; a handler stores and serves each file itself",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/storage_routes/mod.rs", routes_rs(project));
        s.at(Slot::PluginsMod).line("pub mod storage_routes;");
        configurable_hooking(
            s,
            project,
            Hooking {
                key: "storage-routes",
                section: "storage_routes",
                ty: "crate::plugins::storage_routes::StorageRoutes",
                config_ty: "crate::plugins::storage_routes::Settings",
                prelude: ROUTES_COMMENT.to_string(),
                adjust: None,
            },
            routes_settings(),
        );
        s.at(Slot::AgentsSections).block(ROUTES_AGENTS.to_string());
    }
}

const ROUTES_COMMENT: &str = "
        // -- file routes ---------------------------------------------------
        //
        // POST /files stores the `file` part of a form, if `may_upload` in
        // src/plugins/storage_routes/mod.rs says yes. GET /files/{address}
        // serves it to a request with a token the application minted.
        //
";

const ROUTES_AGENTS: &str = "\n\
    ## File routes\n\
    \n\
    Who may upload is `may_upload` in `src/plugins/storage_routes/mod.rs`. It\n\
    answers no until you change it; ask the policy there, not in a page. A\n\
    download needs a token: mint it with `FileStore::tokens` after your policy\n\
    allowed the read, and never log the URL. Do not write an upload or\n\
    download page beside these routes.\n";

fn routes_settings() -> Vec<Setting> {
    vec![
        Setting::integer(
            "max_bytes",
            2_097_152,
            "The largest file accepted, in bytes.",
        )
        .note(
            "The server buffers at most 2 MiB of a request body today, so a larger\n\
             value takes effect only when that limit moves.",
        ),
        Setting::text(
            "csrf_field",
            "_csrf_token",
            "The form field that carries the CSRF token: `[csrf] field`.",
        ),
        Setting::text(
            "csrf_header",
            "x-csrf-token",
            "The header the CSRF check reads: `[csrf] header`.",
        ),
    ]
}

fn routes_rs(project: &Project) -> String {
    let (derive, note) = settings_derive(project, "storage_routes");
    ROUTES_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace("@PATH@", &module_ref("storage-routes", "path"))
        .replace("@PUBLIC@", &module_ref("storage-routes", "public"))
        .replace("@THUMBNAILS@", &module_ref("storage-routes", "thumbnails"))
}

const ROUTES_RS: &str = r##"//! The upload and download routes, as this application answers them.
//!
//! `POST /files` takes a `multipart/form-data` form, stores its `file` part
//! and answers `303` to the form's `return_to` field, with `?file=` and
//! `&token=` added, or `201` with the address. `GET /files/{address}` serves
//! the file to a request whose `?token=` opens it; add `&thumb=300x200` for
//! a thumbnail. The path, `public` and `thumbnails` are in
//! `src/modules/storage_routes.rs`.
//!
//! A plain form works with no script. The CSRF check reads no multipart
//! body, so the `lift` hook copies the form's `_csrf_token` field into the
//! header the check reads, before it runs:
//!
//! ```html
//! <form method="post" action="/files" enctype="multipart/form-data">
//!   <input type="hidden" name="return_to" value="/posts/new">
//!   <input type="file" name="file">
//!   <button>Upload</button>
//! </form>
//! ```
//!
//! A page shows a stored file with a link it mints after its own policy
//! allowed the read:
//!
//! ```ignore
//! let files = ctx.provided::<crate::plugins::storage::FileStore, _>().expect("dispatched");
//! let token = files.tokens.issue(&key);
//! let src = format!("/files/{}?token={}", key.to_hex(), token.as_str());
//! ```

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest, Request, RequestState};
use brocanth::plugin_order::Order;
use brocanth_storage::routes::{FileRoutes, RoutesConfig};
use brocanth_storage::{FileTokens, Storage};

/// Whether this request may store a file.
///
/// It answers no, so nobody can upload until you say who may. The request
/// has passed the session and the CSRF check by now; read the signed-in
/// account from `state` and ask your policy. A "yes" for everyone is a free
/// disk for every stranger.
fn may_upload(_request: &Request, _state: &RequestState) -> bool {
    false
}

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The largest file accepted, in bytes.
    pub max_bytes: usize,
    /// The form field that carries the CSRF token: `[csrf] field`.
    pub csrf_field: String,
    /// The header the CSRF check reads: `[csrf] header`.
    pub csrf_header: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_bytes: 2_097_152,
            csrf_field: "_csrf_token".to_string(),
            csrf_header: "x-csrf-token".to_string(),
        }
    }
}

pub struct StorageRoutes {
    settings: Settings,
    /// Built at `configure`, from the store the storage plugin published.
    routes: Option<FileRoutes>,
}

impl Plugin for StorageRoutes {
    type Config = Settings;
    type Provides = NoContext;
    const NAME: &'static str = "storage_routes";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            routes: None,
        }
    }

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        let store = lc.services().get::<Arc<dyn Storage>>().cloned();
        let tokens = lc.services().get::<FileTokens>().cloned();
        let (Some(store), Some(tokens)) = (store, tokens) else {
            return Err(PluginError::new(
                "[storage_routes] no file store is registered\n\
                 help: register the storage plugin before this one in src/app.rs",
            ));
        };
        let config = RoutesConfig {
            path: @PATH@.to_string(),
            max_bytes: self.settings.max_bytes,
            public: @PUBLIC@,
            thumbnails: @THUMBNAILS@,
            csrf_field: self.settings.csrf_field.clone(),
            csrf_header: self.settings.csrf_header.clone(),
            ..RoutesConfig::default()
        };
        self.routes = Some(FileRoutes::new(store, tokens, config, may_upload)?);
        Ok(())
    }
}

impl HookSource for StorageRoutes {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if let Some(routes) = &self.routes {
            registrar.request_async(
                "lift",
                Order::new().before_if_present("csrf.check"),
                routes.csrf_lift(),
            );
            // After the CSRF check, so an upload from another site stops
            // there, and after the proxy reader and the rate limiter.
            registrar.request_async(
                "answer",
                Order::new()
                    .after_if_present("csrf.check")
                    .after_if_present("proxy-headers.read")
                    .after_if_present("rate_limit.count"),
                routes.clone(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    // Where src/modules/storage_routes.rs puts the routes, so a moved path
    // is tested where it went.
    const PATH: &str = @PATH@;

    #[tokio::test]
    async fn nobody_may_upload_until_may_upload_says_who() {
        let app = crate::app::build().expect("the application builds");
        let mut request = Request::new(Verb::Post, PATH)
            .with_header("content-type", "multipart/form-data; boundary=b");
        request.body = b"--b--\r\n".to_vec().into();
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 403);
    }

    #[tokio::test]
    async fn a_file_is_served_only_with_a_token() {
        let app = crate::app::build().expect("the application builds");
        let address = "0".repeat(64);
        let request = Request::new(Verb::Get, format!("{PATH}/{address}"));
        let response = crate::app::handle(&app, request).await;
        assert_eq!(response.status, 403);
    }
}
"##;
