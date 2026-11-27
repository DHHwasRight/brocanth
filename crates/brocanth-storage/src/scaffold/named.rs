//! What `storage` and `storage-routes` write for named stores
//! (`docs/object-storage.md` §9.1): `src/plugins/storage/stores.rs`, the
//! lines in the two plugin files that open it, and the settings under
//! `[storage.<name>]` and `[storage_routes.<name>]`.
//!
//! A project with no named binding gets none of it: every function here
//! leaves its input as it was for an empty list, so a one-store project
//! keeps its bytes.

use brocanth_scaffold::snippet::settings_derive;
use brocanth_scaffold::{ObjectStore, Project, Scaffold, Setting};

use super::{settings, Backend};
use crate::named::StoreName;

/// One named store: its name and its backend.
#[derive(Clone, Debug)]
pub(super) struct Named {
    pub(super) name: String,
    pub(super) backend: Backend,
}

impl Named {
    /// `public` -> `Public`, for the guard's type.
    fn camel(&self) -> String {
        self.name
            .split('_')
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect()
    }

    /// What the store is, for a doc comment.
    fn what(&self) -> &'static str {
        match self.backend {
            Backend::Fs => "a directory on this machine",
            Backend::Db => "two tables in the `db` database",
            Backend::S3 => "an S3 bucket",
            Backend::Gcs => "a Google Cloud Storage bucket",
        }
    }
}

/// The named stores of `project`, in the order `brocanth.toml` records
/// them. With `s`, each name the rule refuses ([`StoreName`]) is refused
/// there, with a `help:` line; without, it is left out, so only one module
/// says it.
pub(super) fn stores(project: &Project, mut s: Option<&mut Scaffold>) -> Vec<Named> {
    let mut out = Vec::new();
    // Only `storage` asks for the default store: `storage-routes` declares
    // no Kind, and the rule is said once.
    let default = s.as_ref().map(|_| Backend::of(project));
    for (name, key) in project.named::<ObjectStore>() {
        if let Err(why) = StoreName::parse(name) {
            if let Some(s) = s.as_deref_mut() {
                s.refuse(format!(
                    "{why}\nhelp: give the store another name, such as `{key}@{name}_store`"
                ));
            }
            continue;
        }
        let backend = Backend::from_key(key);
        let doubled =
            default == Some(Backend::Db) || out.iter().any(|n: &Named| n.backend == Backend::Db);
        if backend == Backend::Db && doubled {
            if let Some(s) = s.as_deref_mut() {
                s.refuse(format!(
                    "`storage-db` backs two stores, and its tables have fixed names\nhelp: run `brocanth remove {key}@{name}`, and put that store on another backend"
                ));
            }
            continue;
        }
        out.push(Named {
            name: name.to_string(),
            backend,
        });
    }
    out
}

/// A setting key `<name>.<key>`, for the scaffold's `&'static str`. The
/// generator runs a few times per command, so the few bytes it keeps are
/// bounded.
fn key(name: &str, key: &str) -> &'static str {
    Box::leak(format!("{name}.{key}").into_boxed_str())
}

/// The settings of each named store, under `[storage.<name>]`: its
/// backend's, less `secret` and `token_ttl`, which stay in `[storage]`. A
/// disk store defaults to `var/uploads-<name>`; a cloud store has no
/// default bucket (D24).
pub(super) fn storage_settings(stores: &[Named]) -> Vec<Setting> {
    let mut out = Vec::new();
    for store in stores {
        let name = store.name.as_str();
        for setting in settings(store.backend) {
            let named = match setting.key {
                "secret" | "token_ttl" => continue,
                "root" => Setting::text(
                    key(name, "root"),
                    &format!("var/uploads-{name}"),
                    "Where this store's uploads are stored.",
                )
                .note("Its own directory: two stores in one directory stop the start."),
                "bucket" => Setting::text(key(name, "bucket"), "", "The bucket.").note(
                    "No default: name the bucket, or the start stops. Two stores in one\n\
                     bucket and prefix stop the start too.",
                ),
                _ => {
                    let mut moved = setting.clone();
                    moved.key = key(name, setting.key);
                    let flag = format!("--storage.{}", setting.key);
                    moved.note = moved
                        .note
                        .replace(&flag, &format!("--storage.{name}.{}", setting.key));
                    moved
                }
            };
            out.push(named);
        }
    }
    out
}

/// The settings of each named store's routes, under
/// `[storage_routes.<name>]`. `downloads` only for a cloud store, as for
/// the default store.
pub(super) fn routes_settings(stores: &[Named]) -> Vec<Setting> {
    let mut out = Vec::new();
    for store in stores {
        let name = store.name.as_str();
        out.push(
            Setting::text(
                key(name, "path"),
                "",
                "Where this store's routes answer. Empty: under `path`, at `path/<name>`.",
            )
            .note(
                "The name is then part of a public URL: a new name or path breaks the links\n\
                 already sent.",
            ),
        );
        out.push(Setting::bool(
            key(name, "public"),
            false,
            "Serve a file of this store to a request with no token.",
        ));
        out.push(Setting::integer(
            key(name, "max_bytes"),
            2_097_152,
            "The largest file this store accepts, in bytes.",
        ));
        if store.backend.cloud() {
            out.push(
                Setting::text(
                    key(name, "downloads"),
                    "proxy",
                    "How a download of this store is answered: \"proxy\" or \"redirect\".",
                )
                .note(
                    "\"proxy\" sends the bytes from this process. \"redirect\" answers 307 to a URL\n\
                     the bucket signs: keep the bucket private, and let `img-src` allow its origin.",
                ),
            );
            out.push(
                Setting::integer(
                    key(name, "signed_ttl"),
                    300,
                    "Seconds a signed URL of this store's redirect lives, from 1 to 604800.",
                )
                .note("It cannot be revoked, so this is the leak window."),
            );
        }
    }
    out
}

/// The default store's location, as the plugin's `configure` builds it,
/// and the call that opens the named stores.
fn open_call(backend: Backend) -> &'static str {
    match backend {
        Backend::Fs => {
            "        // The named stores, each with its own token key and at a location
        // of its own: two stores at one location stop the start here.
        let at = Location::disk(std::path::Path::new(&self.settings.root));
        let named = stores::open(&self.settings, Some(at), lc)?;
"
        }
        Backend::Db => {
            "        // The named stores, each with its own token key and at a location
        // of its own: two stores at one location stop the start here.
        let named = stores::open(&self.settings, None, lc)?;
"
        }
        Backend::S3 => {
            "        // The named stores, each with its own token key and at a location
        // of its own: two stores at one location stop the start here.
        let at = Location::s3(
            Some(&self.settings.endpoint),
            &self.settings.bucket,
            Some(&self.settings.prefix),
        );
        let named = stores::open(&self.settings, Some(at), lc)?;
"
        }
        Backend::Gcs => {
            "        // The named stores, each with its own token key and at a location
        // of its own: two stores at one location stop the start here.
        let at = Location::gcs(
            Some(&self.settings.endpoint),
            &self.settings.bucket,
            Some(&self.settings.prefix),
        );
        let named = stores::open(&self.settings, Some(at), lc)?;
"
        }
    }
}

/// Puts `add` before the first `anchor` in `text`. Panics when the anchor
/// is not there: the template changed under this function, and a test of
/// the shape says so.
fn before(text: &mut String, anchor: &str, add: &str) {
    let at = text
        .find(anchor)
        .unwrap_or_else(|| panic!("the template has no {anchor:?}"));
    text.insert_str(at, add);
}

fn swap(text: &mut String, from: &str, to: &str) {
    assert!(text.contains(from), "the template has no {from:?}");
    *text = text.replacen(from, to, 1);
}

/// `src/plugins/storage/mod.rs` with the lines that open the named stores.
pub(super) fn plugin_rs(mut text: String, backend: Backend, stores: &[Named]) -> String {
    if stores.is_empty() {
        return text;
    }
    swap(
        &mut text,
        "use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesContext};",
        "use brocanth::plugin_context::{ContextBuilder, ContextItem, ProvidesBoth};",
    );
    let location = if backend == Backend::Db {
        ""
    } else {
        "use brocanth_storage::named::Location;\n"
    };
    before(&mut text, "use brocanth_storage::{", location);
    before(
        &mut text,
        "\n/// What this plugin is built from.",
        "\npub mod stores;\n\npub use stores::NamedStores;\n",
    );
    let mut fields = String::new();
    let mut defaults = String::new();
    for store in stores {
        let name = &store.name;
        let ty = settings_type(store.backend);
        fields.push_str(&format!(
            "    /// The named store `{name}`, {}: `[storage.{name}]`.\n    pub {name}: stores::{ty},\n",
            store.what()
        ));
        defaults.push_str(&match store.backend {
            Backend::Fs => format!(
                "            {name}: stores::DiskSettings {{\n                root: \"var/uploads-{name}\".to_string(),\n            }},\n"
            ),
            _ => format!("            {name}: stores::{ty}::default(),\n"),
        });
    }
    before(
        &mut text,
        "    /// The deployment secret. It signs file tokens",
        &fields,
    );
    before(&mut text, "            secret: \"", &defaults);
    swap(
        &mut text,
        "    tokens: Option<FileTokens>,\n",
        "    tokens: Option<FileTokens>,\n    /// The named stores, opened at `configure` beside the default one.\n    named: Option<NamedStores>,\n",
    );
    swap(
        &mut text,
        "            tokens: None,\n",
        "            tokens: None,\n            named: None,\n",
    );
    swap(
        &mut text,
        "    type Provides = ProvidesContext<FileStore>;",
        "    type Provides = ProvidesBoth<FileStore, NamedStores>;",
    );
    before(
        &mut text,
        "        lc.services_mut().insert(Arc::clone(&store));",
        &format!(
            "{}        lc.services_mut().insert(named.clone());\n        self.named = Some(named);\n",
            open_call(backend)
        ),
    );
    swap(
        &mut text,
        "                tokens: tokens.clone(),\n            });\n        }\n",
        "                tokens: tokens.clone(),\n            });\n        }\n        if let Some(named) = &self.named {\n            cx.insert(named.clone());\n        }\n",
    );
    if stores.iter().any(|s| s.backend == Backend::Db) {
        before(
            &mut text,
            "\n    fn provide(&self, cx: &mut ContextBuilder) {",
            BEFORE_START_NAMED,
        );
    }
    text
}

const BEFORE_START_NAMED: &str = "
    fn before_start(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The pool the `db` plugin opened in its own `before_start`, which
        // runs first: the named Postgres store goes behind its deferred one.
        match &self.named {
            Some(named) => stores::before_start(named, &self.settings, lc),
            None => Ok(()),
        }
    }
";

fn settings_type(backend: Backend) -> &'static str {
    match backend {
        Backend::Fs => "DiskSettings",
        Backend::Db => "DbSettings",
        Backend::S3 => "S3Settings",
        Backend::Gcs => "GcsSettings",
    }
}

/// A `use` line as rustfmt lays it out: one line when it fits in 100
/// columns, else the items wrapped at four spaces.
fn use_line(path: &str, items: &[&str]) -> String {
    let one = format!("use {path}::{{{}}};\n", items.join(", "));
    // rustfmt keeps a `use` line of at most 98 columns whole.
    if one.len() <= 99 {
        return one;
    }
    let mut out = format!("use {path}::{{\n");
    let mut line = String::from("   ");
    for item in items {
        if line.len() + 1 + item.len() + 1 > 100 {
            out.push_str(&line);
            out.push('\n');
            line = String::from("   ");
        }
        line.push(' ');
        line.push_str(item);
        line.push(',');
    }
    out.push_str(&line);
    out.push_str("\n};\n");
    out
}

/// `src/plugins/storage/stores.rs`: one field per named store, the
/// settings type of each backend, and the code that opens each.
pub(super) fn stores_rs(project: &Project, default: Backend, stores: &[Named]) -> String {
    let uses = |b: Backend| stores.iter().any(|s| s.backend == b);
    let cloud = uses(Backend::S3) || uses(Backend::Gcs);
    let db = uses(Backend::Db);
    let mut out = String::from(STORES_HEAD);

    // The imports.
    if db {
        out.push_str("use std::sync::atomic::{AtomicBool, Ordering};\n");
    }
    out.push_str("use std::sync::{Arc, OnceLock};\n");
    out.push('\n');
    out.push_str("use brocanth::plugin_api::{Lifecycle, PluginError};\n");
    out.push_str("use brocanth::plugin_context::ContextItem;\n");
    let mut clouds: Vec<&str> = Vec::new();
    if cloud {
        clouds.push("CloudBucket");
    }
    if uses(Backend::Gcs) {
        clouds.push("GcsConfig");
    }
    if uses(Backend::S3) {
        clouds.push("S3Config");
    }
    if !clouds.is_empty() {
        out.push_str(&use_line("brocanth_storage::cloud", &clouds));
    }
    out.push_str("use brocanth_storage::named::{apart, Location};\n");
    let mut items: Vec<&str> = Vec::new();
    if cloud || db {
        items.push("Deferred");
    }
    items.extend(["FileTokenKey", "FileTokens", "HashKey"]);
    if uses(Backend::Fs) {
        items.push("LocalDisk");
    }
    if db {
        items.push("ObjectKey");
    }
    if cloud {
        items.push("ObjectStorage");
    }
    if db {
        items.push("PgStorage");
    }
    items.push("Storage");
    out.push_str(&use_line("brocanth_storage", &items));

    // The stores.
    out.push_str(
        "\n/// The named stores, one field per name. A handler reads one by its\n\
         /// name, so a store that is not there does not compile.\n\
         #[derive(Clone)]\n\
         // A handler reads the stores: nothing this project generated may.\n\
         #[allow(dead_code)]\n\
         pub struct NamedStores {\n",
    );
    for store in stores {
        out.push_str(&format!(
            "    /// `[storage.{}]`: {}.\n    pub {}: NamedStore,\n",
            store.name,
            store.what(),
            store.name
        ));
    }
    if db {
        out.push_str(
            "    /// The Postgres store waits for the `db` pool: `before_start` puts it\n\
             \x20   /// behind this.\n\
             \x20   pending: Option<Deferred>,\n",
        );
    }
    out.push_str("}\n");
    out.push_str(
        "\nimpl NamedStores {\n\
         \x20   /// The store called `name`, for a caller that holds the name as text:\n\
         \x20   /// `image::src_in`. A handler names the field instead.\n\
         \x20   #[allow(dead_code)]\n\
         \x20   pub fn get(&self, name: &str) -> Option<&NamedStore> {\n\
         \x20       match name {\n",
    );
    for store in stores {
        out.push_str(&format!(
            "            {:?} => Some(&self.{}),\n",
            store.name, store.name
        ));
    }
    out.push_str("            _ => None,\n        }\n    }\n}\n");
    out.push_str(STORES_STRUCTS);

    // The settings of each backend.
    let (derive, _) = settings_derive(project, "storage");
    let file = derive.contains("Deserialize");
    for backend in [Backend::Fs, Backend::Db, Backend::S3, Backend::Gcs] {
        if !uses(backend) {
            continue;
        }
        let secret = backend.cloud();
        let derive = match (file, secret) {
            (true, false) => derive.clone(),
            (true, true) => derive.replace("Debug, ", ""),
            (false, false) => "#[derive(Debug)]".to_string(),
            (false, true) => String::new(),
        };
        let derive = if derive.is_empty() {
            String::new()
        } else {
            format!("{derive}\n")
        };
        out.push_str(
            &settings_struct(backend)
                .replace("@DERIVE@", &derive)
                .replace("@NOTE@", if file { NOTE_FILE } else { NOTE_FLAGS }),
        );
    }

    // Opening them.
    let lc = if cloud { "lc" } else { "_lc" };
    out.push_str(&format!(
        "\n/// Opens every named store, and refuses two stores at one location: the\n\
         /// same directory, or the same endpoint, bucket and prefix. `default` is\n\
         /// where the default store keeps its objects.\n\
         pub(super) fn open(\n    settings: &super::Settings,\n    default: Option<Location>,\n    {lc}: &mut Lifecycle<'_>,\n) -> Result<NamedStores, PluginError> {{\n\
         \x20   let mut at: Vec<(&'static str, Location)> =\n        default.into_iter().map(|at| (\"default\", at)).collect();\n"
    ));
    if db {
        out.push_str("    let mut pending = None;\n");
    }
    for store in stores {
        let name = &store.name;
        let call = match store.backend {
            Backend::Fs => format!("open_disk(\"{name}\", &settings.{name}, settings, &mut at)?"),
            Backend::S3 => format!("open_s3(\"{name}\", &settings.{name}, settings, lc, &mut at)?"),
            Backend::Gcs => {
                format!("open_gcs(\"{name}\", &settings.{name}, settings, lc, &mut at)?")
            }
            Backend::Db => format!("open_db(\"{name}\", settings, &mut pending)"),
        };
        let line = format!("    let {name} = {call};\n");
        if line.len() <= 101 {
            out.push_str(&line);
        } else {
            out.push_str(&format!("    let {name} =\n        {call};\n"));
        }
    }
    out.push_str("    apart(&at).map_err(PluginError::new)?;\n");
    let mut fields: Vec<String> = stores.iter().map(|s| s.name.clone()).collect();
    if db {
        fields.push("pending".to_string());
    }
    let one = format!("    Ok(NamedStores {{ {} }})\n", fields.join(", "));
    // rustfmt puts a struct literal on one line up to 18 columns of body
    // (`struct_lit_width`).
    if fields.join(", ").len() <= 18 {
        out.push_str(&one);
    } else {
        out.push_str("    Ok(NamedStores {\n");
        for field in &fields {
            out.push_str(&format!("        {field},\n"));
        }
        out.push_str("    })\n");
    }
    out.push_str("}\n");
    out.push_str(STORES_COMMON);
    if uses(Backend::Fs) {
        out.push_str(OPEN_DISK);
    }
    if uses(Backend::S3) {
        out.push_str(OPEN_S3);
    }
    if uses(Backend::Gcs) {
        out.push_str(OPEN_GCS);
    }
    if cloud {
        out.push_str(CLOUD_HELPERS);
    }
    if db {
        let name = &stores
            .iter()
            .find(|s| s.backend == Backend::Db)
            .expect("a Postgres store")
            .name;
        out.push_str(OPEN_DB);
        out.push_str(&BEFORE_START_DB.replace("@NAME@", name));
    }
    out.push_str(&stores_tests(default, stores));
    out
}

const NOTE_FILE: &str = "It derives `Deserialize` because this application reads it from a\n/// `[storage.<name>]` table in a TOML file.";

const NOTE_FLAGS: &str = "Nothing deserialises it: `src/plugins/mod.rs` starts from the store's\n/// default and parses each flag and variable into its field.";

fn settings_struct(backend: Backend) -> &'static str {
    match backend {
        Backend::Fs => DISK_SETTINGS,
        Backend::Db => DB_SETTINGS,
        Backend::S3 => S3_SETTINGS,
        Backend::Gcs => GCS_SETTINGS,
    }
}

const STORES_HEAD: &str = r#"//! The named stores, as this application opens them: one field of
//! [`NamedStores`] per name in `[named_bindings]` of `brocanth.toml`
//! (`docs/object-storage.md` §9.1).
//!
//! A handler reads a store by its name, so a typo does not compile:
//!
//! ```ignore
//! let stores = ctx.provided::<NamedStores, _>().expect("a dispatched request");
//! let key = stores.public.store.put(&bytes, &about).await?;
//! let token = stores.public.tokens.issue(&key); // after the policy said yes
//! ```
//!
//! Each store reads `[storage.<name>]`, or `--storage.<name>.<key>`.
//! `secret` and `token_ttl` stay in `[storage]`: each store derives its
//! token key from the one secret with its name in the domain separator, so
//! a token one store minted opens nothing in another. Two stores at one
//! location stop the start, because each would delete the other's objects.
//!
//! `brocanth add <module>@<name>` and `brocanth remove <module>@<name>`
//! change the list, and rewrite this file while nobody has edited it.

"#;

const STORES_STRUCTS: &str = r#"
impl ContextItem for NamedStores {}

/// One named store: the store, and the tokens that open its addresses.
#[derive(Clone)]
// A handler reads the store and its tokens: nothing this project generated
// may.
#[allow(dead_code)]
pub struct NamedStore {
    /// The store, behind the trait, as `FileStore::store` is.
    pub store: Arc<dyn Storage>,
    /// Mints and verifies this store's tokens. A token of another store
    /// opens nothing here.
    pub tokens: FileTokens,
    /// Where the store's routes answer, once `storage-routes` set it at
    /// start. Unset in a project without the routes.
    pub path: Arc<OnceLock<String>>,
}
"#;

const DISK_SETTINGS: &str = r#"
/// A named store in a directory.
///
/// @NOTE@
@DERIVE@pub struct DiskSettings {
    /// Where the objects live, relative to the working directory or
    /// absolute. Created if it is not there.
    pub root: String,
}

impl Default for DiskSettings {
    /// No directory: a `[storage.<name>]` table that names none stops the
    /// start. Each store's own default, `var/uploads-<name>`, is in
    /// `Settings::default`.
    fn default() -> Self {
        Self {
            root: String::new(),
        }
    }
}
"#;

const DB_SETTINGS: &str = r#"
/// A named store in the `db` database.
///
/// @NOTE@
@DERIVE@pub struct DbSettings {
    /// Create the two storage tables at startup if they are absent.
    pub migrate: bool,
}

impl Default for DbSettings {
    fn default() -> Self {
        Self { migrate: true }
    }
}
"#;

const S3_SETTINGS: &str = r#"
/// A named store in an S3 bucket.
///
/// @NOTE@
@DERIVE@pub struct S3Settings {
    /// The S3 bucket. No default: a named store names its bucket.
    pub bucket: String,
    /// The bucket's region. R2 takes `auto`.
    pub region: String,
    /// An S3-compatible server's URL (MinIO, R2). Empty means AWS.
    pub endpoint: String,
    /// Path-style requests, which MinIO wants.
    pub path_style: bool,
    /// The access key. Empty, with the secret: the AWS credential chain.
    pub access_key_id: String,
    /// The secret key. No flag: `--storage.<name>.secret_access_key_file`.
    pub secret_access_key: String,
    /// A folder inside the bucket.
    pub prefix: String,
    /// Check the bucket and the credentials before the server listens.
    pub probe: bool,
    /// The largest object stored, in bytes.
    pub max_object_bytes: u64,
}

impl Default for S3Settings {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            region: "us-east-1".to_string(),
            endpoint: String::new(),
            path_style: true,
            access_key_id: String::new(),
            secret_access_key: String::new(),
            prefix: String::new(),
            probe: true,
            max_object_bytes: 67_108_864,
        }
    }
}

/// Prints `<redacted>` for the secret key: a derived `Debug` would put it
/// in a log.
impl std::fmt::Debug for S3Settings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Settings")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("path_style", &self.path_style)
            .field("access_key_id", &self.access_key_id)
            .field("secret_access_key", &"<redacted>")
            .field("prefix", &self.prefix)
            .field("probe", &self.probe)
            .field("max_object_bytes", &self.max_object_bytes)
            .finish()
    }
}
"#;

const GCS_SETTINGS: &str = r#"
/// A named store in a Google Cloud Storage bucket.
///
/// @NOTE@
@DERIVE@pub struct GcsSettings {
    /// The GCS bucket. No default: a named store names its bucket.
    pub bucket: String,
    /// A service-account key, as JSON. Empty: application default
    /// credentials. No flag: `--storage.<name>.credentials_file`.
    pub credentials: String,
    /// An emulator's URL. Empty means Google.
    pub endpoint: String,
    /// A folder inside the bucket.
    pub prefix: String,
    /// Check the bucket and the credentials before the server listens.
    pub probe: bool,
    /// The largest object stored, in bytes.
    pub max_object_bytes: u64,
}

impl Default for GcsSettings {
    fn default() -> Self {
        Self {
            bucket: String::new(),
            credentials: String::new(),
            endpoint: String::new(),
            prefix: String::new(),
            probe: true,
            max_object_bytes: 67_108_864,
        }
    }
}

/// Prints `<redacted>` for the key: a derived `Debug` would put it in a
/// log.
impl std::fmt::Debug for GcsSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GcsSettings")
            .field("bucket", &self.bucket)
            .field("credentials", &"<redacted>")
            .field("endpoint", &self.endpoint)
            .field("prefix", &self.prefix)
            .field("probe", &self.probe)
            .field("max_object_bytes", &self.max_object_bytes)
            .finish()
    }
}
"#;

const STORES_COMMON: &str = r#"
/// The tokens of the store `name`: the one secret, with the name in the
/// domain separator. The default store's key does not change.
fn tokens(name: &str, settings: &super::Settings) -> FileTokens {
    FileTokens::new(FileTokenKey::from_secret_for(
        settings.secret.as_bytes(),
        name,
    ))
    .with_ttl(settings.token_ttl)
}

/// The address key, as the default store's: an address never leaves the
/// store that holds it.
fn hash_key(settings: &super::Settings) -> HashKey {
    HashKey::from_secret(settings.secret.as_bytes())
}
"#;

const OPEN_DISK: &str = r#"
/// A store in a directory.
fn open_disk(
    name: &'static str,
    store: &DiskSettings,
    settings: &super::Settings,
    at: &mut Vec<(&'static str, Location)>,
) -> Result<NamedStore, PluginError> {
    if store.root.is_empty() {
        return Err(PluginError::new(format!(
            "[storage.{name}] root is required; there is no default directory\n\
             help: set `root` under [storage.{name}], such as \"var/uploads-{name}\""
        )));
    }
    let disk = LocalDisk::open(&store.root)
        .map_err(|e| {
            PluginError::new(format!(
                "[storage.{name}] opening {:?}: {e}\n\
                 help: set `root` under [storage.{name}] to a directory this process can create and write",
                store.root
            ))
        })?
        .with_hash_key(hash_key(settings));
    at.push((name, Location::disk(disk.root())));
    Ok(NamedStore {
        store: Arc::new(disk),
        tokens: tokens(name, settings),
        path: Arc::default(),
    })
}
"#;

const OPEN_S3: &str = r#"
/// A store in an S3 bucket. Builds the client and sends nothing; the
/// bucket and the credentials are checked before the server listens.
fn open_s3(
    name: &'static str,
    store: &S3Settings,
    settings: &super::Settings,
    lc: &mut Lifecycle<'_>,
    at: &mut Vec<(&'static str, Location)>,
) -> Result<NamedStore, PluginError> {
    if store.bucket.is_empty() {
        return Ok(no_bucket(name, settings, lc));
    }
    let bucket = CloudBucket::s3(&S3Config {
        bucket: store.bucket.clone(),
        region: store.region.clone(),
        endpoint: set(&store.endpoint),
        path_style: store.path_style,
        access_key_id: set(&store.access_key_id),
        secret_access_key: set(&store.secret_access_key),
        prefix: set(&store.prefix),
    })
    .map_err(|e| PluginError::new(format!("[storage.{name}] {e}")))?;
    at.push((
        name,
        Location::s3(Some(&store.endpoint), &store.bucket, Some(&store.prefix)),
    ));
    let objects = ObjectStorage::new(Arc::new(bucket))
        .with_hash_key(hash_key(settings))
        .with_max_object_bytes(store.max_object_bytes);
    if store.probe {
        probe(name, objects.clone(), lc);
    }
    Ok(NamedStore {
        store: Arc::new(objects),
        tokens: tokens(name, settings),
        path: Arc::default(),
    })
}
"#;

const OPEN_GCS: &str = r#"
/// A store in a Google Cloud Storage bucket. Builds the client and sends
/// nothing; the bucket and the credentials are checked before the server
/// listens.
fn open_gcs(
    name: &'static str,
    store: &GcsSettings,
    settings: &super::Settings,
    lc: &mut Lifecycle<'_>,
    at: &mut Vec<(&'static str, Location)>,
) -> Result<NamedStore, PluginError> {
    if store.bucket.is_empty() {
        return Ok(no_bucket(name, settings, lc));
    }
    let bucket = CloudBucket::gcs(&GcsConfig {
        bucket: store.bucket.clone(),
        credentials: set(&store.credentials),
        endpoint: set(&store.endpoint),
        prefix: set(&store.prefix),
    })
    .map_err(|e| PluginError::new(format!("[storage.{name}] {e}")))?;
    at.push((
        name,
        Location::gcs(Some(&store.endpoint), &store.bucket, Some(&store.prefix)),
    ));
    let objects = ObjectStorage::new(Arc::new(bucket))
        .with_hash_key(hash_key(settings))
        .with_max_object_bytes(store.max_object_bytes);
    if store.probe {
        probe(name, objects.clone(), lc);
    }
    Ok(NamedStore {
        store: Arc::new(objects),
        tokens: tokens(name, settings),
        path: Arc::default(),
    })
}
"#;

const CLOUD_HELPERS: &str = r#"
/// A cloud store with no bucket. A named store has no default bucket, so
/// two stores never share one: the start stops before the server listens,
/// with the line to add, and the store refuses every call.
fn no_bucket(name: &'static str, settings: &super::Settings, lc: &mut Lifecycle<'_>) -> NamedStore {
    lc.ready_when(move || async move {
        Err::<(), String>(format!(
            "[storage.{name}] bucket is empty; a named store has no default bucket\n\
             help: set `bucket` under [storage.{name}], or --storage.{name}.bucket"
        ))
    });
    NamedStore {
        store: Arc::new(Deferred::new()),
        tokens: tokens(name, settings),
        path: Arc::default(),
    }
}

/// `serve` awaits this once before it listens, so a bucket that is not
/// there or a key it refuses stops the start with a `help:` line.
fn probe(name: &'static str, objects: ObjectStorage, lc: &mut Lifecycle<'_>) {
    lc.ready_when(move || {
        let objects = objects.clone();
        async move {
            objects.probe().await.map_err(|e| {
                let said = e.to_string();
                let said = said.strip_prefix("[storage] ").unwrap_or(&said);
                format!("[storage.{name}] {said}")
            })
        }
    });
}

/// A setting as an option: empty means none.
fn set(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}
"#;

const OPEN_DB: &str = r#"
/// A store in Postgres, on the `db` pool. The pool opens at the `db`
/// plugin's `before_start`, after this, so the store waits behind a
/// deferred one.
fn open_db(name: &str, settings: &super::Settings, pending: &mut Option<Deferred>) -> NamedStore {
    let deferred = Deferred::new();
    *pending = Some(deferred.clone());
    NamedStore {
        store: Arc::new(deferred),
        tokens: tokens(name, settings),
        path: Arc::default(),
    }
}
"#;

const BEFORE_START_DB: &str = r#"
/// Puts the named Postgres store behind its deferred store once the `db`
/// pool is open, and checks its tables before the server listens.
pub(super) fn before_start(
    stores: &NamedStores,
    settings: &super::Settings,
    lc: &mut Lifecycle<'_>,
) -> Result<(), PluginError> {
    let Some(deferred) = &stores.pending else {
        return Ok(());
    };
    let db = lc.services().get::<brocanth_db::Db>().ok_or_else(|| {
        PluginError::new(
            "[storage.@NAME@] found no database pool\n\
             help: register the `db` plugin before storage in src/plugins/mod.rs",
        )
    })?;
    let store = Arc::new(PgStorage::from_pool(db.pool().clone()).with_hash_key(hash_key(settings)));
    deferred.set(store.clone());
    // With `migrate`, the first check creates the tables; without it, they
    // must exist.
    let migrated = Arc::new(AtomicBool::new(!settings.@NAME@.migrate));
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

/// The tests of `stores.rs`: a token of one store is refused by another,
/// always; two stores keep their bytes apart, with two disk stores; and a
/// start with two stores at one location fails, with two disk stores or
/// two buckets of one service.
fn stores_tests(default: Backend, stores: &[Named]) -> String {
    let mut out = String::from(
        "\n#[cfg(test)]\nmod tests {\n    use super::NamedStores;\n    use crate::plugins::storage::FileStore;\n",
    );
    let first = &stores[0].name;
    out.push_str(&format!(
        r#"
    // Async: with `db`, the application builds inside a runtime.
    #[tokio::test]
    async fn a_token_of_one_store_is_refused_by_another() {{
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let files: &FileStore = cx.extract();
        let named: &NamedStores = cx.extract();
        let store = &named.{first};
        let key = brocanth_storage::ObjectKey::of(b"one address");
        let token = files.tokens.issue(&key);
        assert!(store.tokens.verify(token.as_str()).is_err());
        let own = store.tokens.issue(&key);
        assert!(files.tokens.verify(own.as_str()).is_err());
        assert_eq!(store.tokens.verify(own.as_str()).ok(), Some(key));
    }}
"#
    ));
    // The disk stores: the default one first, as `default`.
    let mut disks: Vec<&str> = Vec::new();
    if default == Backend::Fs {
        disks.push("default");
    }
    disks.extend(
        stores
            .iter()
            .filter(|s| s.backend == Backend::Fs)
            .map(|s| s.name.as_str()),
    );
    if disks.len() >= 2 {
        let reach = |name: &str| {
            if name == "default" {
                "files.store".to_string()
            } else {
                format!("named.{name}.store")
            }
        };
        let (a, b) = (reach(disks[0]), reach(disks[1]));
        out.push_str(&format!(
            r#"
    #[tokio::test]
    async fn two_stores_keep_their_bytes_apart() {{
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let files: &FileStore = cx.extract();
        let named: &NamedStores = cx.extract();
        let (one, other) = (&{a}, &{b});
        let (body, about) = (b"kept apart", brocanth_storage::ObjectInfo::new());
        let key = one.put(body, &about).await.expect("stored");
        assert!(!other.exists(&key).await.expect("asked"));
        other.put_at(&key, body, &about).await.expect("stored");
        one.delete(&key).await.expect("deleted");
        assert!(other.exists(&key).await.expect("asked"));
        other.delete(&key).await.expect("deleted");
    }}
"#
        ));
        let field = |name: &str| {
            if name == "default" {
                "settings.root".to_string()
            } else {
                format!("settings.{name}.root")
            }
        };
        let (fa, fb) = (field(disks[0]), field(disks[1]));
        out.push_str(&format!(
            r#"
    #[test]
    fn a_start_with_two_stores_in_one_directory_fails() {{
        use crate::plugins::storage::{{Files, Settings}};
        use brocanth::plugin_api::Plugin;
        use brocanth::plugin_app::AppBuilder;

        let mut settings = Settings::default();
        {fb} = {fa}.clone();
        let built = AppBuilder::new().plugin(Files::new(settings)).build();
        let Err(refused) = built else {{
            panic!("two stores in one directory started");
        }};
        let said = refused.to_string();
        assert!(said.contains("are both at the directory"), "{{said}}");
    }}
"#
        ));
    }
    out.push_str("}\n");
    out
}

/// The lines `storage_routes/mod.rs` gains for the named stores: a guard
/// each, a settings field each, and the routes of each store before the
/// default's.
pub(super) fn routes_rs(mut text: String, project: &Project, stores: &[Named]) -> String {
    if stores.is_empty() {
        return text;
    }
    before(
        &mut text,
        "use brocanth_storage::routes::{",
        "use brocanth_storage::named::AnyRoutes;\n",
    );
    if !text.contains("use brocanth_storage::routes::{Downloads, ") {
        swap(
            &mut text,
            "use brocanth_storage::routes::{",
            "use brocanth_storage::routes::{Downloads, ",
        );
    }
    // `use crate::...` after the crate imports.
    let storage_use = text
        .find("use brocanth_storage::{")
        .expect("the storage import");
    let end = storage_use + text[storage_use..].find(";\n").expect("its end") + 2;
    text.insert_str(
        end,
        "\nuse crate::plugins::storage::stores::NamedStore;\nuse crate::plugins::storage::NamedStores;\n",
    );

    let mut guards = String::new();
    for store in stores {
        let name = &store.name;
        let camel = store.camel();
        guards.push_str(&format!(
            r#"
/// Whether this request may store a file in the store `{name}`.
///
/// It answers no, as `may_upload` does, until you say who may.
async fn may_upload_{name}(_request: &Request, _state: &RequestState) -> bool {{
    false
}}

/// `may_upload_{name}`, as the routes of `{name}` call it.
struct Guard{camel};

impl UploadGuard for Guard{camel} {{
    fn may_upload<'a>(&'a self, request: &'a Request, state: &'a RequestState) -> GuardFuture<'a> {{
        Box::pin(may_upload_{name}(request, state))
    }}
}}
"#
        ));
    }
    before(&mut text, "\n/// What this plugin is built from.", &guards);

    let mut fields = String::new();
    let mut defaults = String::new();
    for store in stores {
        let name = &store.name;
        fields.push_str(&format!(
            "    /// The routes of the store `{name}`: `[storage_routes.{name}]`.\n    pub {name}: StoreRoutes,\n"
        ));
        defaults.push_str(&format!("            {name}: StoreRoutes::default(),\n"));
    }
    swap(
        &mut text,
        "    pub dedupe: bool,\n",
        &format!("    pub dedupe: bool,\n{fields}"),
    );
    swap(
        &mut text,
        "            dedupe: false,\n",
        &format!("            dedupe: false,\n{defaults}"),
    );
    let (derive, note) = settings_derive(project, "storage_routes");
    before(
        &mut text,
        "\npub struct StorageRoutes {",
        &STORE_ROUTES
            .replace("@DERIVE@", &derive)
            .replace("@NOTE@", &note),
    );
    swap(
        &mut text,
        "    routes: Option<FileRoutes>,",
        "    routes: Option<AnyRoutes>,",
    );
    let mut list = String::new();
    let mut built: Vec<String> = Vec::new();
    for store in stores {
        let name = &store.name;
        // Past rustfmt's 60 columns of arguments, so one per line.
        list.push_str(&format!(
            "        let {name}_routes = named_routes(\n            \"{name}\",\n            &self.settings.{name},\n            &config,\n            csp_of_stores.as_deref(),\n            named.{name},\n            Guard{},\n        )?;\n        check_signing(\"{name}\", &{name}_routes, lc);\n",
            store.camel()
        ));
        built.push(format!("{name}_routes"));
    }
    let one = built.join(", ");
    let vec = if one.len() <= 60 {
        format!("vec![{one}]")
    } else {
        let mut v = String::from("vec![\n");
        for b in &built {
            v.push_str(&format!("            {b},\n"));
        }
        v.push_str("        ]");
        v
    };
    // The default store's routes are built last, as `routes` when they may
    // redirect and in one line when they may not.
    let redirect = "        let routes = FileRoutes::new(store, tokens, config, Guard)?;\n";
    let plain = "        self.routes = Some(FileRoutes::new(store, tokens, config, Guard)?);\n";
    let anchor = if text.contains(redirect) {
        redirect
    } else {
        plain
    };
    before(
        &mut text,
        anchor,
        &format!(
            "        // The named stores first: the default's routes answer `POST\n\
             \x20       // {{path}}/<name>` with a 405, as if `<name>` were an address.\n\
             \x20       let Some(named) = lc.services().get::<NamedStores>().cloned() else {{\n\
             \x20           return Err(PluginError::new(\n\
             \x20               \"[storage_routes] no named stores are registered\\n\\\n\
             \x20                help: register the storage plugin before this one in src/app.rs\",\n\
             \x20           ));\n\
             \x20       }};\n\
             \x20       // Each named store that redirects checks its own bucket's origin\n\
             \x20       // against the policy the security headers send (D34).\n\
             \x20       let csp_of_stores = lc\n\
             \x20           .services()\n\
             \x20           .get::<brocanth_middleware::SecurityHeadersConfig>()\n\
             \x20           .and_then(|headers| headers.enforced_csp());\n\
             {list}\x20       let mut all = {vec};\n"
        ),
    );
    if anchor == plain {
        swap(
            &mut text,
            plain,
            "        all.push(FileRoutes::new(store, tokens, config, Guard)?);\n        self.routes = Some(AnyRoutes::new(all));\n",
        );
    } else {
        swap(
            &mut text,
            "        self.routes = Some(routes);\n",
            "        all.push(routes);\n        self.routes = Some(AnyRoutes::new(all));\n",
        );
    }
    before(&mut text, "\n#[cfg(test)]\nmod tests {", NAMED_ROUTES_FNS);
    // The tests: a token of the default store opens nothing in another.
    let names: Vec<String> = stores.iter().map(|s| format!("{:?}", s.name)).collect();
    let test = format!(
        r#"
    #[tokio::test]
    async fn a_token_of_the_default_store_opens_nothing_in_a_named_one() {{
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let files: &crate::plugins::storage::FileStore = cx.extract();
        let key = brocanth_storage::ObjectKey::of(b"one address");
        let token = files.tokens.issue(&key);
        for name in [{names}] {{
            let mut request = Request::new(Verb::Get, format!("{{PATH}}/{{name}}/{{}}", key.to_hex()));
            request.query = format!("token={{}}", token.as_str());
            let response = crate::app::handle(&app, request).await;
            assert_eq!(response.status, 403, "{{name}}");
        }}
    }}
}}
"#,
        names = names.join(", ")
    );
    let mut test = test;
    if let Some(bucket) = stores.iter().find(|s| s.backend == Backend::S3) {
        test = test
            .strip_suffix("}\n")
            .expect("the end of the tests")
            .to_string();
        test.push_str(&redirect_test(bucket, stores));
        test.push_str("}\n");
    }
    let end = text.rfind("}\n").expect("the end of the tests");
    text.replace_range(end.., &test);
    text
}

/// The test that a named S3 store may redirect, and a named disk store,
/// when there is one, may not (D33). A unit test of the routes: a 307
/// needs a bucket, and the project's tests have none.
fn redirect_test(bucket: &Named, stores: &[Named]) -> String {
    let name = &bucket.name;
    let camel = bucket.camel();
    let made = call("made", "named_routes", name, &camel);
    let mut out = format!(
        r#"
    #[test]
    fn a_named_bucket_store_may_redirect() {{
        use super::{{named_routes, StoreRoutes}};
        use crate::plugins::storage::stores::NamedStore;
        use brocanth_storage::cloud::{{CloudBucket, S3Config}};
        use brocanth_storage::routes::RoutesConfig;
        use brocanth_storage::{{FileTokenKey, FileTokens, ObjectStorage}};
        use std::sync::Arc;

        // Built with no request: the client signs, the bucket is not asked.
        let bucket = CloudBucket::s3(&S3Config {{
            bucket: "assets".to_string(),
            region: "us-east-1".to_string(),
            endpoint: Some("http://localhost:9000".to_string()),
            path_style: true,
            access_key_id: Some("key".to_string()),
            secret_access_key: Some("secret".to_string()),
            prefix: None,
        }})
        .expect("a client");
        let store = NamedStore {{
            store: Arc::new(ObjectStorage::new(Arc::new(bucket))),
            tokens: FileTokens::new(FileTokenKey::from_secret_for(b"test", "{name}")),
            path: Arc::default(),
        }};
        let settings = StoreRoutes {{
            downloads: "redirect".to_string(),
            signed_ttl: 60,
            ..StoreRoutes::default()
        }};
        let base = RoutesConfig::default();
{made}        let routes = made.expect("a bucket signs");
        assert!(routes.redirects());
        assert_eq!(routes.path(), "/files/{name}");
"#
    );
    if let Some(disk) = stores.iter().find(|s| s.backend == Backend::Fs) {
        let disk_name = &disk.name;
        let disk_camel = disk.camel();
        out.push_str(&format!(
            r#"
        // A disk cannot sign, so the store refuses `redirect` (D33).
        let disk = brocanth_storage::LocalDisk::open("var/uploads-{disk_name}").expect("a directory");
        let store = NamedStore {{
            store: Arc::new(disk),
            tokens: FileTokens::new(FileTokenKey::from_secret_for(b"test", "{disk_name}")),
            path: Arc::default(),
        }};
{refused}        let said = made.expect_err("a disk does not sign").to_string();
        assert!(said.contains("downloads is \"redirect\""), "{{said}}");
"#,
            refused = call("made", "named_routes", disk_name, &disk_camel)
        ));
    }
    out.push_str("    }\n");
    out
}

/// `let {var} = {fun}("<name>", &settings, &base, None, store, super::Guard<Camel>);`
/// as rustfmt lays it out: one line while the arguments fit in 60 columns.
fn call(var: &str, fun: &str, name: &str, camel: &str) -> String {
    let args = format!("\"{name}\", &settings, &base, None, store, super::Guard{camel}");
    if args.len() <= 60 {
        format!("        let {var} = {fun}({args});\n")
    } else {
        let mut out = format!("        let {var} = {fun}(\n");
        for arg in args.split(", ") {
            out.push_str(&format!("            {arg},\n"));
        }
        out.push_str("        );\n");
        out
    }
}

/// `src/plugins/image/mod.rs` with `src_in`: a link to a thumbnail in a
/// named store, at the path its routes answer.
pub(super) fn image_rs(mut text: String, stores: &[Named]) -> String {
    if stores.is_empty() {
        return text;
    }
    swap(
        &mut text,
        "use crate::plugins::storage::FileStore;\n",
        "use crate::plugins::storage::FileStore;\nuse crate::plugins::storage::NamedStores;\n",
    );
    before(&mut text, "\n#[cfg(test)]\nmod tests {", IMAGE_SRC_IN);
    let first = &stores[0].name;
    before(
        &mut text,
        "    fn get(target: &str) -> Request {",
        &IMAGE_SRC_IN_TEST.replace("@NAME@", first),
    );
    text
}

const IMAGE_SRC_IN: &str = r#"
/// The link to one thumbnail of `key` in the named store `store`: `<img
/// src>`. The path is the one that store's routes answer at, as
/// `[storage_routes.<name>] path` set it.
#[allow(dead_code)]
pub fn src_in(
    ctx: &Ctx,
    store: &str,
    key: &ObjectKey,
    size: &str,
    format: Option<&str>,
) -> Result<String, String> {
    let stores = ctx
        .provided::<NamedStores, _>()
        .ok_or_else(|| "this Ctx has no named stores: it was built by hand".to_string())?;
    let named = stores
        .get(store)
        .ok_or_else(|| format!("no store is named `{store}`"))?;
    let path = named
        .path
        .get()
        .ok_or_else(|| format!("the store `{store}` has no routes"))?;
    let token = named.tokens.issue(key);
    Ok(routes::thumb_url(path, key, &token, spec(size, format)?))
}
"#;

const IMAGE_SRC_IN_TEST: &str = r#"    #[tokio::test]
    async fn a_named_store_links_to_its_own_routes() {
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let stores: &NamedStores = cx.extract();
        let store = stores.get("@NAME@").expect("the store");
        let at = format!("{PATH}/@NAME@");
        assert_eq!(store.path.get(), Some(&at));
        assert!(stores.get("nowhere").is_none());
    }

"#;

const STORE_ROUTES: &str = r#"
/// The routes of one named store: `[storage_routes.<name>]`. The rest, the
/// thumbnails and `dedupe` among them, is the default store's.
///
/// @NOTE@
@DERIVE@
pub struct StoreRoutes {
    /// Where the routes answer. Empty: `{path}/<name>`.
    pub path: String,
    /// Serve a file to a request with no token.
    pub public: bool,
    /// The largest file accepted, in bytes.
    pub max_bytes: usize,
    /// How a download is answered: "proxy" or "redirect".
    pub downloads: String,
    /// Seconds a signed URL of a redirect lives.
    pub signed_ttl: u64,
}

impl Default for StoreRoutes {
    fn default() -> Self {
        Self {
            path: String::new(),
            public: false,
            max_bytes: 2_097_152,
            downloads: "proxy".to_string(),
            signed_ttl: 300,
        }
    }
}
"#;

const NAMED_ROUTES_FNS: &str = r#"
/// The routes of the named store `name`: the default's configuration, with
/// the store's own path, `public`, `max_bytes`, `downloads` and
/// `signed_ttl`. A store that cannot sign, a disk or Postgres, refuses
/// `redirect` here (D33).
fn named_routes(
    name: &str,
    settings: &StoreRoutes,
    base: &RoutesConfig,
    csp: Option<&str>,
    store: NamedStore,
    guard: impl UploadGuard,
) -> Result<FileRoutes, PluginError> {
    let path = if settings.path.is_empty() {
        format!("{}/{name}", base.path.trim_end_matches('/'))
    } else {
        settings.path.clone()
    };
    let config = RoutesConfig {
        path,
        public: settings.public,
        max_bytes: settings.max_bytes,
        downloads: downloads(name, &settings.downloads)?,
        signed_ttl: std::time::Duration::from_secs(settings.signed_ttl),
        csp: csp.map(str::to_string),
        ..base.clone()
    };
    let routes = FileRoutes::new(store.store, store.tokens, config, guard)
        .map_err(|e| PluginError::new(in_store(name, &e.to_string())))?;
    // `image::src_in` links to the path the routes answer at.
    let _ = store.path.set(routes.path().to_string());
    Ok(routes)
}

/// How the downloads of the named store `name` are answered: `proxy`, the
/// bytes from this process, or `redirect`, a 307 to a URL the bucket signs.
fn downloads(name: &str, value: &str) -> Result<Downloads, PluginError> {
    Downloads::parse(value).map_err(|e| PluginError::new(in_store(name, &e)))
}

/// A message of the routes, about the named store `name`: its section is
/// `[storage_routes.<name>]`.
fn in_store(name: &str, said: &str) -> String {
    said.replace("[storage_routes]", &format!("[storage_routes.{name}]"))
}

/// The start check of a named store that redirects (D34): `serve` awaits
/// it once before it listens, so a key that cannot sign, or an `img-src`
/// that blocks this store's bucket, stops the start with a `help:` line.
fn check_signing(name: &'static str, routes: &FileRoutes, lc: &mut Lifecycle<'_>) {
    if routes.redirects() {
        let check = routes.clone();
        lc.ready_when(move || {
            let check = check.clone();
            async move {
                let said = check.check_signing().await;
                said.map_err(|e| format!("[storage_routes.{name}] {e}"))
            }
        });
    }
}
"#;

/// The `AGENTS.md` lines for the named stores, after the storage section.
pub(super) fn agents(stores: &[Named]) -> String {
    if stores.is_empty() {
        return String::new();
    }
    let first = &stores[0].name;
    let list: Vec<String> = stores
        .iter()
        .map(|s| format!("`{}` ({})", s.name, s.what()))
        .collect();
    format!(
        "\nNamed stores: {}. Reach one with\n\
         `ctx.provided::<NamedStores, _>()?.{first}.store`, and mint its tokens with\n\
         `.{first}.tokens`: a token of one store opens nothing in another. Each\n\
         reads `[storage.<name>]`; `secret` and `token_ttl` stay in `[storage]`.\n",
        list.join(", ")
    )
}
