//! `storage`, `storage-routes` and `storage-thumbnails` -- what `brocanth
//! create` writes when somebody asks to keep files.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: `LocalDisk::open`, `PgStorage::from_pool`,
//! that [`with_hash_key`](crate::LocalDisk::with_hash_key) is the difference
//! between an address anybody can compute and one nobody can, that
//! [`FileTokenKey`](crate::FileTokenKey) and [`HashKey`](crate::HashKey)
//! derive separately from one secret, and that opening a store is
//! synchronous. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! # Where the objects live
//!
//! On a local disk under `[storage] root`, or, with `db`, in the `db`
//! database on its pool ([`PgStorage`](crate::PgStorage)). The db pool opens
//! at the `db` plugin's `before_start`, after the file routes are built, so
//! the plugin publishes a [`Deferred`](crate::Deferred) store at `configure`
//! and puts the Postgres store behind it at `before_start`.
//!
//! # What it does not write
//!
//! **An upload route or a download route.** Those are the child
//! [`StorageRoutes`], `storage-routes`, which a project asks for. Minting a
//! file token stays a decision about *this* application's policy, and so
//! does who may upload: the child writes an `UploadGuard` that answers no
//! until the project edits it, and no route mints a token on its own.
//!
//! **Image variants.** `variants` pulls four image decoders in, and §8.2 asks
//! that a feature which is off means the code is absent. So the decoders
//! come only with the grandchild [`StorageThumbnails`],
//! `storage-thumbnails`, which a project asks for. `create` merges the
//! features every module asks of one crate into one dependency line.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{
    configurable, configurable_hooking, module_ref, redact_secrets, settings_derive, Hooking,
};
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, MountRole, Opt, PgPool,
    Project, Scaffold, Setting, Slot,
};

/// The scaffold's development secret. `brocanth check --deploy` finds it by
/// its first words, and the generated plugin refuses it outside
/// development.
const DEVELOPMENT_SECRET: &str = "development only: replace before anything is uploaded in anger";

/// File storage, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct FileStorage;

impl Module for FileStorage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage",
            "uploads on a local disk, or in Postgres with `db`, addressed by content hash; no S3",
        )
        .category(Category::Enhancer)
        .category(Category::Middleware)
        .uses::<PgPool>(
            "the objects live in the `db` database, on its pool, and not on one machine's disk",
        )
        .asked_in(
            "data",
            "Uploads on disk or in Postgres, addressed by content hash",
        )
        .krate(Dep::brocanth("brocanth-storage").feature_with("db", "postgres"))
        .absent(
            "brocanth-storage",
            "no file storage; an upload has nowhere to go and no address to be read back from",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let db = project.provider::<PgPool>().is_some();
        s.file(
            "src/plugins/storage/mod.rs",
            redact_secrets(&plugin_rs(project, db), &settings(db)),
        );
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
        configurable(s, project, "storage", "Files", settings(db));
        // No `.gitignore` line: `LocalDisk::open` writes one holding `*` into
        // the root, so the rule moves when `root` does.
    }
}

/// The `[storage]` settings: where the objects live, the one secret, and the
/// token's life.
///
/// The secret has no flag. A flag is in `ps` output for every user on the
/// host, and this one mints download links for every object.
fn settings(db: bool) -> Vec<Setting> {
    let mut out = Vec::new();
    if db {
        out.push(
            Setting::bool(
                "migrate",
                true,
                "Create the two storage tables at startup if they are absent.",
            )
            .note(
                "Development only: see `PgStorage::migrate` and `brocanth_storage::postgres::sql`\n\
                 for the statements to put in a migration instead.",
            ),
        );
    } else {
        out.push(
            Setting::text("root", "var/uploads", "Where uploads are stored.").note(
                "Created with a `.gitignore` that holds `*`, so an upload is never source.\n\
                 It is one machine's disk: a second instance does not see these files. Add\n\
                 the `db` module and the files live in Postgres instead.",
            ),
        );
    }
    out.push(
        Setting::text(
            "secret",
            DEVELOPMENT_SECRET,
            "Signs file tokens and keys object addresses.",
        )
        .note(
            "Development only, and the one line to change first. While it is this\n\
             string, anybody who has read this repository can mint a token for any\n\
             object; outside development the plugin refuses to start with it. No flag\n\
             sets it: a flag is readable by every user on the host.",
        )
        .secret(),
    );
    out.push(
        Setting::integer("token_ttl", 120, "Seconds a file token lives.").note(
            "It cannot be revoked, so this number is the window in which a URL pasted\n\
             into a chat still works. Two minutes is long enough for a page full of\n\
             images and short enough to be dead before anyone clicks it.",
        ),
    );
    out
}

fn plugin_rs(project: &Project, db: bool) -> String {
    let (derive, note) = settings_derive(project, "storage");
    let pick = |local: &'static str, on_db: &'static str| if db { on_db } else { local };
    PLUGIN_RS
        .replace("@NOTE@", &note)
        .replace("@DERIVE@", &derive)
        .replace("@SECRET@", DEVELOPMENT_SECRET)
        .replace("@WHERE@", pick(WHERE_LOCAL, WHERE_DB))
        .replace("@STD_USES@", pick(STD_USES_LOCAL, STD_USES_DB))
        .replace("@STORAGE_ITEMS@", pick(ITEMS_LOCAL, ITEMS_DB))
        .replace("@FIELDS@", pick(FIELDS_LOCAL, FIELDS_DB))
        .replace("@DEFAULTS@", pick(DEFAULTS_LOCAL, DEFAULTS_DB))
        .replace("@PLUGIN_FIELD@", pick("", PLUGIN_FIELD_DB))
        .replace("@PLUGIN_NONE@", pick("", PLUGIN_NONE_DB))
        .replace("@ROOT_CHECK@", pick(ROOT_CHECK, ""))
        .replace("@OPEN@", pick(OPEN_LOCAL, OPEN_DB))
        .replace("@KEEP@", pick("", KEEP_DB))
        .replace("@BEFORE_START@", pick("", BEFORE_START_DB))
}

const WHERE_LOCAL: &str = "\
//! The objects live under `root`, on this machine's disk. A second instance
//! does not see them, and `brocanth check --deploy` says so: add the `db`
//! module and they live in Postgres instead.
";

const WHERE_DB: &str = "\
//! The objects live in the `db` database, in `bytea` columns, on the `db`
//! plugin's pool: every instance sees every file, and a backup of the
//! database holds them.
";

const STD_USES_LOCAL: &str = "use std::sync::Arc;\n";

const STD_USES_DB: &str = "use std::sync::atomic::{AtomicBool, Ordering};\nuse std::sync::Arc;\n";

const ITEMS_LOCAL: &str = "FileTokenKey, FileTokens, HashKey, LocalDisk, Storage";

/// One name a line would pass 100 columns, so rustfmt breaks the list.
const ITEMS_DB: &str =
    "\n    Deferred, FileTokenKey, FileTokens, HashKey, ObjectKey, PgStorage, Storage,\n";

const FIELDS_LOCAL: &str =
    "    /// Where the objects live, relative to the working directory or absolute.
    /// Created if it is not there.
    pub root: String,
";

const FIELDS_DB: &str = "    /// Create the two storage tables at startup if they are absent.
    pub migrate: bool,
";

const DEFAULTS_LOCAL: &str = "            root: \"var/uploads\".to_string(),\n";

const DEFAULTS_DB: &str = "            migrate: true,\n";

const PLUGIN_FIELD_DB: &str =
    "    /// The store the routes hold; the Postgres store goes behind it at\n    /// `before_start`, once the `db` pool is open.\n    deferred: Option<Deferred>,\n";

const PLUGIN_NONE_DB: &str = "            deferred: None,\n";

const ROOT_CHECK: &str = r#"        if self.settings.root.is_empty() {
            return Err(PluginError::new(
                "[storage] root is required; there is no default directory\n\
                 help: set `root` under [storage] in the config file, such as \"var/uploads\"",
            ));
        }
"#;

const OPEN_LOCAL: &str = r#"        // Synchronous: `std::fs`, a `create_dir_all` and a `canonicalize`.
        // Unlike a pool there is nothing to open, so there is no
        // `before_start`.
        let disk = LocalDisk::open(&self.settings.root)
            .map_err(|e| {
                PluginError::new(format!(
                    "[storage] opening {:?}: {e}\n\
                     help: set `root` under [storage] to a directory this process can create and write",
                    self.settings.root
                ))
            })?
            .with_hash_key(HashKey::from_secret(self.settings.secret.as_bytes()));
        let store: Arc<dyn Storage> = Arc::new(disk);
"#;

const OPEN_DB: &str = r#"        // The `db` pool opens at the `db` plugin's `before_start`, after the
        // routes are built. So the routes hold this deferred store, and
        // `before_start` below puts the Postgres store behind it.
        let deferred = Deferred::new();
        let store: Arc<dyn Storage> = Arc::new(deferred.clone());
"#;

const KEEP_DB: &str = "        self.deferred = Some(deferred);\n";

const BEFORE_START_DB: &str = r#"
    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The pool the `db` plugin opened in its own `before_start`, which
        // runs first because that plugin is registered first.
        let db = lc.services().get::<brocanth_db::Db>().ok_or_else(|| {
            PluginError::new(
                "[storage] found no database pool\n\
                 help: register the `db` plugin before storage in src/plugins/mod.rs",
            )
        })?;
        let store = Arc::new(
            PgStorage::from_pool(db.pool().clone())
                .with_hash_key(HashKey::from_secret(self.settings.secret.as_bytes())),
        );
        if let Some(deferred) = &self.deferred {
            deferred.set(store.clone());
        }
        // Ready when one read of the tables answers. With `migrate`, the
        // first check creates them, so a new project starts; a project with
        // reviewers turns it off and keeps the statements in a migration.
        // `serve` awaits this once before it listens.
        let migrated = Arc::new(AtomicBool::new(!self.settings.migrate));
        lc.ready_when(move || {
            let store = Arc::clone(&store);
            let migrated = Arc::clone(&migrated);
            async move {
                if !migrated.load(Ordering::Acquire) {
                    store
                        .migrate()
                        .await
                        .map_err(|e| format!("creating the storage tables: {e}"))?;
                    migrated.store(true, Ordering::Release);
                }
                let probe = ObjectKey::of(b"brocanth.readyz");
                store.exists(&probe).await.map(drop).map_err(|e| {
                    format!(
                        "{e}\nhelp: check `[db] url`; with `migrate = false`, the storage \
                         tables must exist: `brocanth_storage::postgres::sql` has the statements"
                    )
                })
            }
        });
        Ok(())
    }
"#;

/// `src/plugins/storage/mod.rs`, with the backend's parts as `@NAME@`.
const PLUGIN_RS: &str = r##"//! File storage, as this application registers it.
//!
//! An object is named by what is in it. Thirty-two bytes rendered as
//! sixty-four hex characters is the whole address, so `..`, a slash and a
//! drive letter are not values the type can hold and no filename a client
//! sent ever becomes a path component. The original name is kept as metadata,
//! defanged for the `Content-Disposition` header it ends up in, and never
//! touches the filesystem.
//!
@WHERE@//!
//! `brocanth-storage` is in `Cargo.toml` because this file is here, and
//! through no other route. Delete the registration in `src/plugins/mod.rs`
//! and delete this file, and the application stores no files and links
//! nothing to store them with.
//!
//! # Uploading and serving
//!
//! The `storage-routes` module writes the HTTP edge: an upload route behind a
//! guard this project writes, and a download route behind a file token.
//! Without it, a handler that has already asked its own policy stores a file:
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
//! A file token says "the bearer may read this object" and carries no
//! identity at all. Mint one only for a request that has already passed the
//! policy, because after that the token is the only thing between the object
//! and whoever the URL reaches.
//!
//! ```ignore
//! let token = files.tokens.issue(&key);           // after the policy said yes
//! let key = files.tokens.verify(presented)?;      // on the download route
//! let meta = files.store.meta(&key).await?;
//! ```
//!
//! # Thumbnails
//!
//! `get_variant` and `put_variant` are here already: moving derived bytes
//! needs no codec, so this build can serve a variant another process
//! computed. The `storage-thumbnails` module turns on the `variants` feature,
//! which adds `derive_variant` and png, jpeg, gif and webp decoders pointed
//! at stranger-supplied bytes. An application that stores documents should
//! leave it off.
//!
//! # One secret, two keys
//!
//! `secret` below is used twice and the two uses cannot be confused for each
//! other: `HashKey` and `FileTokenKey` derive from it under different domain
//! separators, so an address key cannot sign a token and a token key cannot
//! address an object. Rotating the secret invalidates every outstanding token
//! at once, which is the break-glass, and it also changes the address new
//! uploads are stored at -- objects already stored keep the address they
//! were written under, so rotate it with a plan rather than on a Friday.
//! Outside development the plugin refuses the scaffold's secret and any
//! secret shorter than 32 characters.

@STD_USES@
use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};
use brocanth_storage::{@STORAGE_ITEMS@};

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
@FIELDS@    /// The deployment secret. It signs file tokens and keys object addresses;
    /// see the module header for what each of those means.
    pub secret: String,
    /// How many seconds a file token lives. A token cannot be revoked, so
    /// this is the leak window.
    pub token_ttl: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
@DEFAULTS@            secret: "@SECRET@".to_string(),
            token_ttl: 120,
        }
    }
}

/// The scaffold's secret, which the repository shows to anybody.
const DEVELOPMENT_SECRET: &str = "@SECRET@";

/// Whether this process runs in development or a test: `$<APP>_ENV` unset,
/// `development` or `test`. The variable the config plugin reads too.
fn development() -> bool {
    let var = format!("{}_ENV", crate::app::APP.to_uppercase().replace('-', "_"));
    match std::env::var(var) {
        Ok(env) => env == "development" || env == "test",
        Err(_) => true,
    }
}

/// Whether `secret` may sign tokens here. In development anything goes;
/// elsewhere not the scaffold's secret, which this repository shows to
/// anybody, and not one shorter than 32 characters.
fn usable_secret(secret: &str, development: bool) -> bool {
    development || (secret != DEVELOPMENT_SECRET && secret.len() >= 32)
}

/// What this plugin puts on the context.
///
/// A wrapper and not the two values loose, because `ContextItem` is
/// brocanth's trait and both of these are `brocanth-storage`'s types, so the
/// impl that joins the two belongs in this crate, which names both.
pub struct FileStore {
    /// The store, behind the trait, so swapping the backend is a change in
    /// this file and nowhere else.
    ///
    /// Nothing this project generated reads it: a handler does, with
    /// `ctx.provided::<FileStore, _>()`. Delete this attribute when one does.
    #[allow(dead_code)]
    pub store: Arc<dyn Storage>,
    /// Mints and verifies the short-lived tokens a download route checks.
    pub tokens: FileTokens,
}

impl ContextItem for FileStore {}

pub struct Files {
    settings: Settings,
    /// Built at `configure`, because opening a store can fail and
    /// `Plugin::new` cannot.
    store: Option<Arc<dyn Storage>>,
    tokens: Option<FileTokens>,
@PLUGIN_FIELD@}

impl Plugin for Files {
    type Config = Settings;
    type Provides = ProvidesContext<FileStore>;
    const NAME: &'static str = "storage";

    fn new(config: Settings) -> Self {
        Self {
            settings: config,
            store: None,
            tokens: None,
@PLUGIN_NONE@        }
    }

    fn configure(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
@ROOT_CHECK@        if self.settings.secret.is_empty() {
            // An empty secret is not a weaker store, it is two different
            // stores: addresses stop being keyed, and a token key of no bytes
            // signs tokens anybody can forge.
            return Err(PluginError::new(
                "[storage] secret is required; it signs file tokens and keys object addresses\n\
                 help: set `secret` under [storage], or the variable for `storage.secret`, to a long random string",
            ));
        }
        if !usable_secret(&self.settings.secret, development()) {
            return Err(PluginError::new(
                "[storage] secret is the scaffold's development value or shorter than 32 characters\n\
                 help: set the variable for `storage.secret` to 32 random bytes, such as the output of `head -c 32 /dev/urandom | base64`",
            ));
        }
        if self.settings.token_ttl <= 0 {
            return Err(PluginError::new(
                "[storage] token_ttl must be at least one second, or no token is ever valid\n\
                 help: set `token_ttl` under [storage] to 1 or more seconds",
            ));
        }
@OPEN@        let tokens = FileTokens::new(FileTokenKey::from_secret(self.settings.secret.as_bytes()))
            .with_ttl(self.settings.token_ttl);
        // On the services too, where a plugin registered after this one
        // reads them: the upload and download routes of `storage-routes` do.
        lc.services_mut().insert(Arc::clone(&store));
        lc.services_mut().insert(tokens.clone());
        self.store = Some(store);
        self.tokens = Some(tokens);
@KEEP@        Ok(())
    }
@BEFORE_START@
    fn provide(&self, cx: &mut ContextBuilder) {
        // `configure` runs before the server is handed the application, so
        // both are here by the time any request builds a context.
        if let (Some(store), Some(tokens)) = (&self.store, &self.tokens) {
            cx.insert(FileStore {
                store: store.clone(),
                tokens: tokens.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{usable_secret, DEVELOPMENT_SECRET};

    #[test]
    fn the_development_secret_is_refused_outside_development() {
        assert!(usable_secret(DEVELOPMENT_SECRET, true));
        assert!(!usable_secret(DEVELOPMENT_SECRET, false));
        assert!(!usable_secret("short", false));
        assert!(usable_secret(&"x".repeat(32), false));
    }
}
"##;

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
            "POST /files and GET /files/{address}?token=: uploads your guard allows, downloads behind a file token",
        )
        .contributes::<AgentsSection>(Contribution::new("storage-routes", "which file routes exist, and how a file is served"), true)
        .category(Category::Middleware)
        .part_of_but_off("storage")
        .because("it is the HTTP edge of `brocanth-storage`, which the parent brings")
        .asked_in("data", "Upload and download routes, behind a guard and file tokens")
        .adapts_to(
            "storage-thumbnails",
            "`?thumb=WxH` answers in the sizes `thumb_sizes` lists",
        )
        // An upload is a form post. Without the check, any page on any site
        // posts files here with the visitor's cookies. The routes also read
        // the check's field and header, so they keep no copy of either.
        .needs(
            "middleware-csrf",
            "an upload is a form post: without the CSRF check any site posts files here with the visitor's cookies, and the routes read its field and header",
        )
        // `routes` is the HTTP edge. The image decoders come with
        // `storage-thumbnails` only.
        .krate(Dep::brocanth("brocanth-storage").feature("routes"))
        // The plugin reads the store and the token service from the
        // services the storage plugin fills, at its own `after_config`.
        .after("storage")
        .hook(
            "answer",
            "answer POST {path} and GET {path}/{address}",
            Hook::required().phase(brocanth_scaffold::Phase::Cache),
        )
        .hook(
            "lift",
            "copy an upload form's CSRF field into the header the CSRF check reads",
            Hook::on().phase(brocanth_scaffold::Phase::Guard),
        )
        .option(
            Opt::mount(
                "path",
                "/files",
                "Where the routes answer: uploads at {path}, files at {path}/{address}.",
            )
            // Uploads and downloads, not pages.
            .role(MountRole::Machine),
        )
        .option(Opt::bool(
            "public",
            false,
            "Serve a file to a request with no token. Off, every download needs a token.",
        ))
        .absent(
            "storage-routes",
            "no upload or download route; a handler stores and serves each file itself",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let thumbnails = project.has("storage-thumbnails");
        s.file(
            "src/plugins/storage_routes/mod.rs",
            routes_rs(project, thumbnails),
        );
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
            routes_settings(thumbnails),
        );
        s.at(Slot::AgentsSections).block(ROUTES_AGENTS.to_string());
    }
}

/// Thumbnails on the file routes: `storage-thumbnails`, a child of
/// `storage` that needs `storage-routes`. Its key starts with its parent's,
/// as every child key does.
///
/// It writes no file. It turns on the `variants` feature, which links the
/// image decoders, and `storage-routes` then answers `?thumb=` in the sizes
/// its `thumb_sizes` setting lists. A feature and not an option, because an
/// option is read at run time and the decoders are linked at build time.
pub struct StorageThumbnails;

impl Module for StorageThumbnails {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-thumbnails",
            "?thumb=WxH on the file routes, in listed sizes, rendered once each; links four image decoders",
        )
        .category(Category::Middleware)
        .part_of_but_off("storage")
        .because("it renders the stored images, which the parent brings")
        .asked_in("data", "")
        .needs(
            "storage-routes",
            "the thumbnails answer `?thumb=` on the file routes, so without them nothing asks for one",
        )
        .krate(Dep::brocanth("brocanth-storage").feature("variants"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
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
    answers no until you change it. It is async: ask the policy there, not in\n\
    a page. A download needs a token: mint it with `FileStore::tokens` after\n\
    your policy allowed the read, and never log the URL. Do not write an\n\
    upload or download page beside these routes.\n";

fn routes_settings(thumbnails: bool) -> Vec<Setting> {
    let mut out = vec![
        Setting::integer(
            "max_bytes",
            2_097_152,
            "The largest file accepted, in bytes.",
        )
        .note(
            "The server buffers at most 2 MiB of a request body today, so a larger\n\
             value takes effect only when that limit moves.",
        ),
        Setting::bool(
            "dedupe",
            false,
            "Store identical uploads once, at their content address.",
        )
        .note(
            "Off, each upload has an address of its own. On, a delete removes the file\n\
             for every holder: count the references before you delete.",
        ),
    ];
    if thumbnails {
        out.push(
            Setting::list(
                "thumb_sizes",
                crate::spec::DEFAULT_THUMB_SIZES,
                "The only sizes ?thumb= may ask for; any other is a 400.",
            )
            .note(
                "WxH crops to the middle, WxHt to the top, WxHb to the bottom, WxHf fits,\n\
                 Wx0 and 0xH scale. Each size is rendered once per file and kept.",
            ),
        );
    }
    out
}

fn routes_rs(project: &Project, thumbnails: bool) -> String {
    let (derive, note) = settings_derive(project, "storage_routes");
    let pick = |off: &'static str, on: &'static str| if thumbnails { on } else { off };
    ROUTES_RS
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace(
            "@THUMB_DOC@",
            pick(
                "",
                "\n//! Add `&thumb=300x200` for a thumbnail, in a size `thumb_sizes` lists.",
            ),
        )
        .replace("@SPEC_USE@", pick("", ", VariantSpec"))
        .replace("@THUMB_FIELD@", pick("", THUMB_FIELD))
        .replace("@THUMB_DEFAULT@", pick("", THUMB_DEFAULT))
        .replace("@THUMB_PARSE@", pick("", THUMB_PARSE))
        .replace(
            "@THUMBNAILS@",
            pick(
                "thumbnails: false,",
                "thumbnails: true,\n            thumb_sizes,",
            ),
        )
        .replace("@PATH@", &module_ref("storage-routes", "path"))
        .replace("@PUBLIC@", &module_ref("storage-routes", "public"))
}

const THUMB_FIELD: &str = "    /// The only sizes `?thumb=` may ask for; any other is a 400.
    pub thumb_sizes: Vec<String>,
";

const THUMB_DEFAULT: &str = "            thumb_sizes: brocanth_storage::routes::DEFAULT_THUMB_SIZES
                .iter()
                .map(|size| size.to_string())
                .collect(),
";

const THUMB_PARSE: &str = r#"        let thumb_sizes = self
            .settings
            .thumb_sizes
            .iter()
            .map(|size| {
                VariantSpec::parse(size).map_err(|e| {
                    PluginError::new(format!("[storage_routes] thumb_sizes `{size}`: {e}"))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
"#;

const ROUTES_RS: &str = r##"//! The upload and download routes, as this application answers them.
//!
//! `POST /files` takes a `multipart/form-data` form, stores its `file` part
//! and answers `303` to the form's `return_to` field, with `?file=` and
//! `&token=` added, or `201` with the address. `GET /files/{address}` serves
//! the file to a request whose `?token=` opens it.@THUMB_DOC@
//! The path and `public` are in `src/modules/storage_routes.rs`.
//!
//! A plain form works with no script. The CSRF check reads no multipart
//! body, so the `lift` hook copies the form's CSRF field into the header the
//! check reads, before it runs. The field and the header are the `[csrf]`
//! ones: this plugin reads them from the CSRF plugin and keeps no copy.
//!
//! ```html
//! <form method="post" action="/files" enctype="multipart/form-data">
//!   <input type="hidden" name="return_to" value="/posts/new">
//!   <input type="file" name="file">
//!   <button>Upload</button>
//! </form>
//! ```
//!
//! Each upload is stored at an address of its own, unless `dedupe` is on.
//! With `dedupe`, identical files share one object, and a `delete` removes
//! it for every holder: count the references before you delete.
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
use brocanth::plugin_order::{Order, Phase};
use brocanth_middleware::CsrfConfig;
use brocanth_storage::routes::{FileRoutes, GuardFuture, RoutesConfig, UploadGuard};
use brocanth_storage::{FileTokens, Storage@SPEC_USE@};

/// Whether this request may store a file.
///
/// It answers no, so nobody can upload until you say who may. The request
/// has passed the session and the CSRF check by now; read the signed-in
/// account from `state` and ask your policy, which may await a row. A "yes"
/// for everyone is a free disk for every stranger.
async fn may_upload(_request: &Request, _state: &RequestState) -> bool {
    false
}

/// `may_upload`, as the routes call it.
struct Guard;

impl UploadGuard for Guard {
    fn may_upload<'a>(&'a self, request: &'a Request, state: &'a RequestState) -> GuardFuture<'a> {
        Box::pin(may_upload(request, state))
    }
}

/// What this plugin is built from.
///
/// @NOTE@
@DERIVE@
pub struct Settings {
    /// The largest file accepted, in bytes.
    pub max_bytes: usize,
    /// Store identical uploads once, at their content address.
    pub dedupe: bool,
@THUMB_FIELD@}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_bytes: 2_097_152,
            dedupe: false,
@THUMB_DEFAULT@        }
    }
}

pub struct StorageRoutes {
    settings: Settings,
    /// Built at `after_config`, from the store the storage plugin published.
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

    fn after_config(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // After every plugin registered: the store and the tokens are the
        // storage plugin's, the field and the header the CSRF plugin's.
        let store = lc.services().get::<Arc<dyn Storage>>().cloned();
        let tokens = lc.services().get::<FileTokens>().cloned();
        let (Some(store), Some(tokens)) = (store, tokens) else {
            return Err(PluginError::new(
                "[storage_routes] no file store is registered\n\
                 help: register the storage plugin before this one in src/app.rs",
            ));
        };
        let Some(csrf) = lc.services().get::<CsrfConfig>() else {
            return Err(PluginError::new(
                "[storage_routes] no CSRF check is registered, so any site could post files here\n\
                 help: register `brocanth_middleware::Csrf` with `plugin_hooking` in src/plugins/mod.rs",
            ));
        };
        let Some(csrf_header) = csrf.header.clone() else {
            return Err(PluginError::new(
                "[storage_routes] [csrf] header is unset, so an upload form's token has no way to the check\n\
                 help: set `header` under [csrf], such as \"x-csrf-token\"",
            ));
        };
@THUMB_PARSE@        let config = RoutesConfig {
            path: @PATH@.to_string(),
            max_bytes: self.settings.max_bytes,
            public: @PUBLIC@,
            @THUMBNAILS@
            dedupe: self.settings.dedupe,
            csrf_field: csrf.field.clone(),
            csrf_header,
            ..RoutesConfig::default()
        };
        self.routes = Some(FileRoutes::new(store, tokens, config, Guard)?);
        Ok(())
    }
}

impl HookSource for StorageRoutes {
    type Families = OnAsyncRequest;

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if let Some(routes) = &self.routes {
            // `Guard`: the CSRF check refuses last in that phase, so the
            // token this hook copies out of the form is there when it
            // reads.
            registrar.request_async("lift", Phase::Guard, Order::new(), routes.csrf_lift());
            // `Cache`, after every guard: an upload from another site
            // stops at the CSRF check, and a flood at the rate limiter.
            registrar.request_async("answer", Phase::Cache, Order::new(), routes.clone());
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
