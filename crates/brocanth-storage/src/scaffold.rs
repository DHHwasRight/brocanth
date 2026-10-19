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
//! **An upload route, a download route, or the policy that guards them.**
//! This crate parses no multipart body and knows nothing about HTTP, and the
//! generator does not invent what the crate refuses: minting a file token is
//! a decision about *this* application's policy, and a generated route that
//! guessed it would be a generated hole. The plugin puts the store and the
//! token service on the context, and the generated file's own header says
//! what the two calls are.
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

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

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
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "storage", "Files"));
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
        if project.has("config") {
            s.at(Slot::ConfigSections).block(SECTION.to_string());
        }
    }
}

/// The `[storage]` section, with the two numbers and the one secret that
/// decide what the store is.
const SECTION: &str = "\n[storage]\n\
# Below the working directory, so an upload is not source and `.gitignore`\n\
# already says so. Move it off the deployment's disk and every instance stops\n\
# seeing the same files: an object store is a separate crate behind the same\n\
# trait, and swapping to it is a change in src/plugins/storage/mod.rs alone.\n\
root = \"var/uploads\"\n\
# Development only, and the one line to change first. It signs file tokens and\n\
# it keys the address an object is stored at. While it is this string, anybody\n\
# who has read this repository can mint a token for any object, and anybody\n\
# holding a copy of a file can compute whether somebody uploaded it.\n\
secret = \"development only: replace before anything is uploaded in anger\"\n\
# Seconds a file token lives. It cannot be revoked, so this number is the\n\
# window in which a URL pasted into a chat still works. Two minutes is long\n\
# enough for a page full of images and short enough to be dead before anyone\n\
# clicks it.\n\
token_ttl = 120\n";

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
//! through no other route. Delete the registration in `src/app.rs` and delete
//! this file, and the application stores no files and links nothing to store
//! them with.
//!
//! # Uploading
//!
//! There is no upload route here, on purpose: `brocanth-storage` parses no
//! multipart body and knows nothing about HTTP, and whether *this*
//! application lets *this* request store a file is a policy question a
//! generator cannot answer. A handler that has already asked it writes:
//!
//! ```ignore
//! let files: &FileStore = cx.extract();
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

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
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
        self.store = Some(Arc::new(disk));
        self.tokens = Some(
            FileTokens::new(FileTokenKey::from_secret(self.settings.secret.as_bytes()))
                .with_ttl(self.settings.token_ttl),
        );
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
