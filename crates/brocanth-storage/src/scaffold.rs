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
//! In the project's object store, the module bound to the `ObjectStore`
//! Kind (`docs/object-storage.md`): `storage-fs`, a directory under
//! `[storage] root` and the default; `storage-s3` and `storage-gcs`, a
//! bucket ([`CloudBucket`](crate::cloud) behind
//! [`ObjectStorage`](crate::ObjectStorage)); `storage-db`, the `db`
//! database on its pool ([`PgStorage`](crate::PgStorage)). The backend
//! modules write no file: this one writes the open code for the bound one.
//! The db pool opens at the `db` plugin's `before_start`, after the file
//! routes are built, so the plugin publishes a
//! [`Deferred`](crate::Deferred) store at `configure` and puts the Postgres
//! store behind it at `before_start`.
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
    AgentsSection, Category, Contribution, Dep, Hook, Manifest, Module, MountRole, ObjectStore,
    Opt, PgPool, Project, Scaffold, Setting, Slot,
};

/// The scaffold's development secret. `brocanth check --deploy` finds it by
/// its first words, and the generated plugin refuses it outside
/// development.
const DEVELOPMENT_SECRET: &str = "development only: replace before anything is uploaded in anger";

/// Which backend holds the objects: the module bound to `ObjectStore`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Backend {
    Fs,
    Db,
    S3,
    Gcs,
}

impl Backend {
    /// The provider of `ObjectStore` in `project`. All four are this
    /// crate's own modules, so the branch stays inside one crate
    /// (`docs/module-capabilities.md` §9). An unresolved selection, which
    /// only a test builds, writes the disk store.
    fn of(project: &Project) -> Self {
        match project.provider::<ObjectStore>() {
            Some("storage-db") => Self::Db,
            Some("storage-s3") => Self::S3,
            Some("storage-gcs") => Self::Gcs,
            _ => Self::Fs,
        }
    }

    fn cloud(self) -> bool {
        matches!(self, Self::S3 | Self::Gcs)
    }
}

/// File storage, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct FileStorage;

impl Module for FileStorage {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage",
            "uploads addressed by content hash, in the project's object store: a disk, S3, GCS or Postgres",
        )
        .category(Category::Enhancer)
        .category(Category::Middleware)
        .requires::<ObjectStore>(
            "the files live in the project's object store; with none chosen, a directory on this machine",
        )
        .contributes::<AgentsSection>(
            Contribution::new("storage", "where the files live, and how code reaches them"),
            true,
        )
        .asked_in("data", "Uploads, addressed by content hash")
        .krate(Dep::brocanth("brocanth-storage"))
        .absent(
            "brocanth-storage",
            "no file storage; an upload has nowhere to go and no address to be read back from",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let backend = Backend::of(project);
        s.file(
            "src/plugins/storage/mod.rs",
            redact_secrets(&plugin_rs(project, backend), &settings(backend)),
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
        configurable(s, project, "storage", "Files", settings(backend));
        s.at(Slot::AgentsSections).block(agents(backend));
        // No `.gitignore` line: `LocalDisk::open` writes one holding `*` into
        // the root, so the rule moves when `root` does.
    }
}

/// `storage-fs`: the objects in a directory. The default `ObjectStore`.
///
/// It writes no file: `storage` writes `LocalDisk` into its own plugin
/// when this module is bound. One machine's disk fits one binary with
/// embedded Postgres; a second instance does not see the files, and
/// `brocanth check --deploy` says so.
pub struct StorageFs;

impl Module for StorageFs {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-fs",
            "the object store is a directory on this machine's disk; the default",
        )
        .category(Category::Enhancer)
        .part_of_but_off("storage")
        .because("it is where the parent's files live; the parent writes the code that opens it")
        .provides::<ObjectStore>("objects in a directory under `[storage] root`")
        .default_for::<ObjectStore>()
        .asked_in("data", "Uploads on this machine's disk")
        .krate(Dep::brocanth("brocanth-storage"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// `storage-db`: the objects in Postgres, in `bytea` columns, on the `db`
/// pool. What `storage` with `db` wrote before the `ObjectStore` Kind.
pub struct StorageDb;

impl Module for StorageDb {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-db",
            "the object store is two tables in the project's Postgres database",
        )
        .tables(&["brocanth_object_variants", "brocanth_objects"])
        .category(Category::Enhancer)
        .part_of_but_off("storage")
        .because("it is where the parent's files live; the parent writes the code that opens it")
        .provides::<ObjectStore>("objects in `bytea` columns, on the `db` pool")
        .requires::<PgPool>(
            "the objects live in the `db` database, on its pool, and not on one machine's disk",
        )
        .asked_in("data", "Uploads in Postgres")
        .krate(Dep::brocanth("brocanth-storage").feature("postgres"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// `storage-s3`: the objects in an S3 bucket, or an S3-compatible one
/// (MinIO, R2).
pub struct StorageS3;

impl Module for StorageS3 {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-s3",
            "the object store is an S3 bucket, or one on MinIO or R2; checked at start",
        )
        .category(Category::Enhancer)
        .part_of_but_off("storage")
        .because("it is where the parent's files live; the parent writes the code that opens it")
        .provides::<ObjectStore>("objects in an S3 bucket, under `[storage] prefix`")
        .asked_in("data", "Uploads in an S3 bucket (or MinIO, R2)")
        .krate(Dep::brocanth("brocanth-storage").feature("s3"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// `storage-gcs`: the objects in a Google Cloud Storage bucket.
pub struct StorageGcs;

impl Module for StorageGcs {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "storage-gcs",
            "the object store is a Google Cloud Storage bucket; checked at start",
        )
        .category(Category::Enhancer)
        .part_of_but_off("storage")
        .because("it is where the parent's files live; the parent writes the code that opens it")
        .provides::<ObjectStore>("objects in a GCS bucket, under `[storage] prefix`")
        .asked_in("data", "Uploads in a Google Cloud Storage bucket")
        .krate(Dep::brocanth("brocanth-storage").feature("gcs"))
    }

    fn contribute(&self, _project: &Project, _s: &mut Scaffold) {}
}

/// The `AGENTS.md` section: one store, behind the trait, and the module
/// that moves it.
fn agents(backend: Backend) -> String {
    let (here, next) = match backend {
        Backend::Fs => (
            "a directory on this machine\n(`storage-fs`, `[storage] root`)",
            "`brocanth add storage-s3` (or `storage-gcs`,\n`storage-db`) moves new uploads",
        ),
        Backend::Db => (
            "Postgres, on the `db` pool\n(`storage-db`)",
            "`brocanth add storage-s3` (or `storage-gcs`)\nmoves new uploads to a bucket",
        ),
        Backend::S3 => (
            "an S3 bucket\n(`storage-s3`, `[storage] bucket`)",
            "`brocanth remove storage-s3` returns to\na directory",
        ),
        Backend::Gcs => (
            "a Google Cloud Storage bucket\n(`storage-gcs`, `[storage] bucket`)",
            "`brocanth remove storage-gcs` returns to\na directory",
        ),
    };
    format!(
        "\n## Files\n\
         \n\
         Uploads live in {here}.\n\
         {next}.\n\
         Code reaches them through `FileStore::store`, an `Arc<dyn Storage>`:\n\
         never name `LocalDisk`, `PgStorage` or `CloudBucket` outside\n\
         `src/plugins/storage/mod.rs`. Keep the address (`key.to_hex()`) on your\n\
         record. A signed URL or a file token is a bearer credential: mint one\n\
         only after your policy allowed the read. Secrets come from\n\
         `--storage.<key>_file`, never a flag.\n"
    )
}

/// The `[storage]` settings: where the objects live, the one secret, and the
/// token's life.
///
/// The secrets have no flag. A flag is in `ps` output for every user on the
/// host; `--storage.<key>_file` and `--storage.<key>_env` read them instead.
fn settings(backend: Backend) -> Vec<Setting> {
    let mut out = Vec::new();
    match backend {
        Backend::Db => out.push(
            Setting::bool(
                "migrate",
                true,
                "Create the two storage tables at startup if they are absent.",
            )
            .note(
                "Development only: see `PgStorage::migrate` and `brocanth_storage::postgres::sql`\n\
                 for the statements to put in a migration instead.",
            ),
        ),
        Backend::Fs => out.push(
            Setting::text("root", "var/uploads", "Where uploads are stored.").note(
                "Created with a `.gitignore` that holds `*`, so an upload is never source.\n\
                 It is one machine's disk: a second instance does not see these files. Add\n\
                 `storage-s3`, `storage-gcs` or `storage-db` and the files live there instead.",
            ),
        ),
        Backend::S3 => {
            out.push(
                Setting::text("bucket", "uploads", "The S3 bucket.")
                    .note("It must exist: the start checks it before the server listens."),
            );
            out.push(Setting::text(
                "region",
                "us-east-1",
                "The bucket's region. R2 takes `auto`.",
            ));
            out.push(
                Setting::text("endpoint", "", "An S3-compatible server's URL.").note(
                    "MinIO, R2: such as \"http://localhost:9000\". Empty means AWS. An http:// URL\n\
                     allows plain HTTP.",
                ),
            );
            out.push(Setting::bool(
                "path_style",
                true,
                "Path-style requests (endpoint/bucket/key), which MinIO wants.",
            ));
            out.push(
                Setting::text("access_key_id", "", "The access key.").note(
                    "Empty, with an empty secret_access_key: the AWS credential chain answers\n\
                     (the AWS_* variables, a web identity token, the instance role).",
                ),
            );
            out.push(
                Setting::text("secret_access_key", "", "The secret key.")
                    .note("No flag: read it with --storage.secret_access_key_file=PATH.")
                    .secret(),
            );
        }
        Backend::Gcs => {
            out.push(
                Setting::text("bucket", "uploads", "The GCS bucket.")
                    .note("It must exist: the start checks it before the server listens."),
            );
            out.push(
                Setting::text("credentials", "", "A service-account key, as JSON.")
                    .note(
                        "No flag: read it with --storage.credentials_file=PATH. Empty: the\n\
                         application default credentials (workload identity, the metadata server).",
                    )
                    .secret(),
            );
            out.push(Setting::text(
                "endpoint",
                "",
                "An emulator's URL, such as \"http://localhost:4443\". Empty means Google.",
            ));
        }
    }
    if backend.cloud() {
        out.push(Setting::text(
            "prefix",
            "",
            "A folder inside the bucket, so two applications share one.",
        ));
        out.push(
            Setting::bool(
                "probe",
                true,
                "Check the bucket and the credentials before the server listens.",
            )
            .note("Off for a start with no network; the first upload then finds the problem."),
        );
        out.push(Setting::integer(
            "max_object_bytes",
            67_108_864,
            "The largest object stored, in bytes.",
        ));
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

fn plugin_rs(project: &Project, backend: Backend) -> String {
    let (derive, note) = settings_derive(project, "storage");
    // Each part: the disk, Postgres, S3, GCS.
    let pick = |parts: [&'static str; 4]| match backend {
        Backend::Fs => parts[0],
        Backend::Db => parts[1],
        Backend::S3 => parts[2],
        Backend::Gcs => parts[3],
    };
    PLUGIN_RS
        .replace("@NOTE@", &note)
        .replace("@DERIVE@", &derive)
        .replace("@SECRET@", DEVELOPMENT_SECRET)
        .replace(
            "@WHERE@",
            pick([WHERE_LOCAL, WHERE_DB, WHERE_S3, WHERE_GCS]),
        )
        .replace(
            "@STD_USES@",
            pick([STD_USES_LOCAL, STD_USES_DB, STD_USES_LOCAL, STD_USES_LOCAL]),
        )
        .replace(
            "@STORAGE_ITEMS@",
            pick([ITEMS_LOCAL, ITEMS_DB, ITEMS_CLOUD, ITEMS_CLOUD]),
        )
        .replace("@CLOUD_USE@", pick(["", "", CLOUD_USE_S3, CLOUD_USE_GCS]))
        .replace(
            "@FIELDS@",
            pick([FIELDS_LOCAL, FIELDS_DB, FIELDS_S3, FIELDS_GCS]),
        )
        .replace(
            "@DEFAULTS@",
            pick([DEFAULTS_LOCAL, DEFAULTS_DB, DEFAULTS_S3, DEFAULTS_GCS]),
        )
        .replace("@HELPERS@", pick(["", "", HELPERS_CLOUD, HELPERS_CLOUD]))
        .replace("@PLUGIN_FIELD@", pick(["", PLUGIN_FIELD_DB, "", ""]))
        .replace("@PLUGIN_NONE@", pick(["", PLUGIN_NONE_DB, "", ""]))
        .replace("@ROOT_CHECK@", pick([ROOT_CHECK, "", "", ""]))
        .replace("@OPEN@", pick([OPEN_LOCAL, OPEN_DB, OPEN_S3, OPEN_GCS]))
        .replace("@KEEP@", pick(["", KEEP_DB, "", ""]))
        .replace("@BEFORE_START@", pick(["", BEFORE_START_DB, "", ""]))
}

const WHERE_LOCAL: &str = "\
//! The objects live under `root`, on this machine's disk. A second instance
//! does not see them, and `brocanth check --deploy` says so: add the
//! `storage-s3`, `storage-gcs` or `storage-db` module and they live there.
";

const WHERE_DB: &str = "\
//! The objects live in the `db` database, in `bytea` columns, on the `db`
//! plugin's pool: every instance sees every file, and a backup of the
//! database holds them.
";

const WHERE_S3: &str = "\
//! The objects live in an S3 bucket, or one on MinIO or R2, under `prefix`:
//! every instance sees every file. The bucket and the credentials are
//! checked before the server listens; `probe = false` skips that for a
//! start with no network.
";

const WHERE_GCS: &str = "\
//! The objects live in a Google Cloud Storage bucket, under `prefix`: every
//! instance sees every file. The bucket and the credentials are checked
//! before the server listens; `probe = false` skips that for a start with
//! no network.
";

const STD_USES_LOCAL: &str = "use std::sync::Arc;\n";

const STD_USES_DB: &str = "use std::sync::atomic::{AtomicBool, Ordering};\nuse std::sync::Arc;\n";

const ITEMS_LOCAL: &str = "FileTokenKey, FileTokens, HashKey, LocalDisk, Storage";

/// One name a line would pass 100 columns, so rustfmt breaks the list.
const ITEMS_DB: &str =
    "\n    Deferred, FileTokenKey, FileTokens, HashKey, ObjectKey, PgStorage, Storage,\n";

const ITEMS_CLOUD: &str = "FileTokenKey, FileTokens, HashKey, ObjectStorage, Storage";

const CLOUD_USE_S3: &str = "use brocanth_storage::cloud::{CloudBucket, S3Config};\n";

const CLOUD_USE_GCS: &str = "use brocanth_storage::cloud::{CloudBucket, GcsConfig};\n";

const FIELDS_LOCAL: &str =
    "    /// Where the objects live, relative to the working directory or absolute.
    /// Created if it is not there.
    pub root: String,
";

const FIELDS_DB: &str = "    /// Create the two storage tables at startup if they are absent.
    pub migrate: bool,
";

const FIELDS_S3: &str = "    /// The S3 bucket.
    pub bucket: String,
    /// The bucket's region. R2 takes `auto`.
    pub region: String,
    /// An S3-compatible server's URL (MinIO, R2). Empty means AWS.
    pub endpoint: String,
    /// Path-style requests, which MinIO wants.
    pub path_style: bool,
    /// The access key. Empty, with the secret: the AWS credential chain.
    pub access_key_id: String,
    /// The secret key. No flag: `--storage.secret_access_key_file`.
    pub secret_access_key: String,
    /// A folder inside the bucket.
    pub prefix: String,
    /// Check the bucket and the credentials before the server listens.
    pub probe: bool,
    /// The largest object stored, in bytes.
    pub max_object_bytes: u64,
";

const FIELDS_GCS: &str = "    /// The GCS bucket.
    pub bucket: String,
    /// A service-account key, as JSON. Empty: application default
    /// credentials. No flag: `--storage.credentials_file`.
    pub credentials: String,
    /// An emulator's URL. Empty means Google.
    pub endpoint: String,
    /// A folder inside the bucket.
    pub prefix: String,
    /// Check the bucket and the credentials before the server listens.
    pub probe: bool,
    /// The largest object stored, in bytes.
    pub max_object_bytes: u64,
";

const DEFAULTS_LOCAL: &str = "            root: \"var/uploads\".to_string(),\n";

const DEFAULTS_DB: &str = "            migrate: true,\n";

const DEFAULTS_S3: &str = "            bucket: \"uploads\".to_string(),
            region: \"us-east-1\".to_string(),
            endpoint: String::new(),
            path_style: true,
            access_key_id: String::new(),
            secret_access_key: String::new(),
            prefix: String::new(),
            probe: true,
            max_object_bytes: 67_108_864,
";

const DEFAULTS_GCS: &str = "            bucket: \"uploads\".to_string(),
            credentials: String::new(),
            endpoint: String::new(),
            prefix: String::new(),
            probe: true,
            max_object_bytes: 67_108_864,
";

const HELPERS_CLOUD: &str = "
/// A setting as an option: empty means none.
fn set(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}
";

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

const OPEN_S3: &str = r#"        // Builds the client and sends nothing: a missing bucket or a bad
        // endpoint stops the start here, and the bucket and the
        // credentials are checked before the server listens, below.
        let bucket = CloudBucket::s3(&S3Config {
            bucket: self.settings.bucket.clone(),
            region: self.settings.region.clone(),
            endpoint: set(&self.settings.endpoint),
            path_style: self.settings.path_style,
            access_key_id: set(&self.settings.access_key_id),
            secret_access_key: set(&self.settings.secret_access_key),
            prefix: set(&self.settings.prefix),
        })
        .map_err(|e| PluginError::new(e.to_string()))?;
        let objects = ObjectStorage::new(Arc::new(bucket))
            .with_hash_key(HashKey::from_secret(self.settings.secret.as_bytes()))
            .with_max_object_bytes(self.settings.max_object_bytes);
        // `serve` awaits this once before it listens, so a bucket that is
        // not there or a key it refuses stops the start with a `help:` line.
        if self.settings.probe {
            let probe = objects.clone();
            lc.ready_when(move || {
                let probe = probe.clone();
                // The check's report names the plugin already.
                async move {
                    probe.probe().await.map_err(|e| {
                        let said = e.to_string();
                        said.strip_prefix("[storage] ").unwrap_or(&said).to_string()
                    })
                }
            });
        }
        let store: Arc<dyn Storage> = Arc::new(objects);
"#;

const OPEN_GCS: &str = r#"        // Builds the client and sends nothing: a missing bucket or a key
        // that is not JSON stops the start here, and the bucket and the
        // credentials are checked before the server listens, below.
        let bucket = CloudBucket::gcs(&GcsConfig {
            bucket: self.settings.bucket.clone(),
            credentials: set(&self.settings.credentials),
            endpoint: set(&self.settings.endpoint),
            prefix: set(&self.settings.prefix),
        })
        .map_err(|e| PluginError::new(e.to_string()))?;
        let objects = ObjectStorage::new(Arc::new(bucket))
            .with_hash_key(HashKey::from_secret(self.settings.secret.as_bytes()))
            .with_max_object_bytes(self.settings.max_object_bytes);
        // `serve` awaits this once before it listens, so a bucket that is
        // not there or a key it refuses stops the start with a `help:` line.
        if self.settings.probe {
            let probe = objects.clone();
            lc.ready_when(move || {
                let probe = probe.clone();
                // The check's report names the plugin already.
                async move {
                    probe.probe().await.map_err(|e| {
                        let said = e.to_string();
                        said.strip_prefix("[storage] ").unwrap_or(&said).to_string()
                    })
                }
            });
        }
        let store: Arc<dyn Storage> = Arc::new(objects);
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
@CLOUD_USE@use brocanth_storage::{@STORAGE_ITEMS@};

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
@HELPERS@
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
        .adapts_to(
            "image",
            "`&fmt=` converts a thumbnail to a format `formats` lists, and `max_pixels` caps the source",
        )
        .adapts_to(
            "storage-s3",
            "`downloads` and `signed_ttl`: a download may redirect to a signed URL of the bucket",
        )
        .adapts_to(
            "storage-gcs",
            "`downloads` and `signed_ttl`: a download may redirect to a signed URL of the bucket",
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
        // `image` needs `storage-thumbnails`, so it never comes alone.
        let image = thumbnails && project.has("image");
        // `downloads` and `signed_ttl` only for a bucket: a disk or Postgres
        // store cannot sign, so the setting would be a mistake there, and
        // the disk and Postgres projects stay as they were
        // (`docs/object-storage.md` §9.3). A module, not the `ObjectStore`
        // provider: one provider holds the Kind, so the two say the same,
        // and `uses::<ObjectStore>` would name the default store in the
        // `adapted_to` of every project with routes.
        let cloud = project.has("storage-s3") || project.has("storage-gcs");
        s.file(
            "src/plugins/storage_routes/mod.rs",
            routes_rs(project, thumbnails, image, cloud),
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
            routes_settings(thumbnails, image, cloud),
        );
        let mut agents = ROUTES_AGENTS.to_string();
        if cloud {
            agents.push_str(ROUTES_AGENTS_REDIRECT);
        }
        s.at(Slot::AgentsSections).block(agents);
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

/// Images: `image`. Format conversion for the thumbnails, a lower pixel
/// cap, and a `srcset` helper.
///
/// The rendering is the crate's: `storage-thumbnails` already links the
/// `image` crate's decoders, and the three encoders this module offers come
/// with them. So the module adds no dependency. It turns on two settings of
/// `storage-routes` (`formats`, `max_pixels`), which adapts to it, and
/// writes `src/plugins/image/mod.rs`, the helper a page calls to link a
/// thumbnail.
pub struct Image;

impl Module for Image {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "image",
            "thumbnails converted on request (&fmt=webp), a pixel cap against decompression bombs, a srcset helper",
        )
        .category(Category::Renderer)
        .contributes::<AgentsSection>(
            Contribution::new("image", "how a page links an image in a size and a format"),
            true,
        )
        .needs(
            "storage-thumbnails",
            "it converts the thumbnails `storage-thumbnails` renders; without them there is no image to convert",
        )
        .after("storage-routes")
        .uses::<ObjectStore>(
            "the test that stores a PNG runs on `storage-fs` only: Postgres, S3 and GCS are services the tests do not have",
        )
        .asked_in("data", "Images: thumbnails in WebP or JPEG, a srcset helper")
        .absent(
            "image",
            "no format conversion; a thumbnail is JPEG or PNG, and a page writes its own srcset",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // The disk is the one store a test has without a service. An
        // unresolved selection, which only a test builds, is the disk too.
        let convert = match project.provider::<ObjectStore>() {
            None | Some("storage-fs") => IMAGE_CONVERT_TEST,
            Some(_) => "",
        };
        s.file(
            "src/plugins/image/mod.rs",
            IMAGE_RS
                .replace("@PATH@", &module_ref("storage-routes", "path"))
                .replace("@CONVERT@", convert),
        );
        s.at(Slot::PluginsMod).line("pub mod image;");
        s.at(Slot::AgentsSections).block(IMAGE_AGENTS.to_string());
    }
}

/// The test that stores a PNG and asks for it in WebP. Left out when the
/// store is not the disk: a generated project's tests have no service.
const IMAGE_CONVERT_TEST: &str = r#"    /// A PNG of one red pixel.
    const PNG: [u8; 69] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x38,
        0x21, 0x27, 0x07, 0x00, 0x02, 0xb6, 0x01, 0x05, 0x34, 0xa6, 0x75, 0xaa, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[tokio::test]
    async fn a_thumbnail_converts_to_a_listed_format() {
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let files: &FileStore = cx.extract();
        let key = files
            .store
            .put(&PNG, &brocanth_storage::ObjectInfo::new())
            .await
            .expect("stored");
        let token = files.tokens.issue(&key);
        let link = |size: &str, format: Option<&str>| {
            routes::thumb_url(
                PATH,
                &key,
                &token,
                spec(size, format).expect("a valid spec"),
            )
        };

        let webp = crate::app::handle(&app, get(&link("150x150t", Some("webp")))).await;
        assert_eq!(webp.status, 200);
        assert_eq!(webp.header("content-type"), Some("image/webp"));
    }

"#;

const IMAGE_AGENTS: &str = "\n\
    ## Images\n\
    \n\
    Link a stored image through `crate::plugins::image`: `src(ctx, &key,\n\
    size, format)` for one `<img src>`, `srcset(ctx, &key, sizes, format)`\n\
    for `<img srcset>`. Call them after your policy allowed the read: each\n\
    link carries a file token. A size must be in `thumb_sizes` and a format\n\
    in `formats` under `[storage_routes]`, or the link answers 400; add the\n\
    size there, never an open-ended width parameter. Use `webp` for a logo\n\
    or a screenshot and `jpeg` for a photograph: WebP here is lossless.\n";

const IMAGE_RS: &str = r##"//! Links to stored images, in a size and a format the file routes allow.
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! ```ignore
//! let src = crate::plugins::image::src(ctx, &key, "300x200", Some("webp"))?;
//! let set = crate::plugins::image::srcset(ctx, &key, &["150x150t", "600x600f"], Some("webp"))?;
//! // <img src="{src}" srcset="{set}" sizes="(max-width: 600px) 150px, 600px" alt="…">
//! ```
//!
//! Each link carries a file token, so mint it after your policy allowed
//! the read. A size must be in `thumb_sizes` and a format in `formats`
//! under `[storage_routes]`: the routes answer any other with a 400 before
//! they read the file, so nobody can make the server render a size no page
//! uses. A source past `max_pixels` is a 413, read from its header before a
//! pixel is decoded.

use brocanth_storage::routes;
use brocanth_storage::{ObjectKey, VariantFormat, VariantSpec};

use crate::pages::Ctx;
use crate::plugins::storage::FileStore;

/// Where the file routes answer: `path` in `src/modules/storage_routes.rs`.
const PATH: &str = @PATH@;

/// One size and format, as `routes` spells them, or why not.
fn spec(size: &str, format: Option<&str>) -> Result<VariantSpec, String> {
    let spec = VariantSpec::parse(size).map_err(|e| format!("size `{size}`: {e}"))?;
    let format = format
        .map(|f| VariantFormat::parse(f).map_err(|e| format!("format `{f}`: {e}")))
        .transpose()?;
    Ok(spec.with_format(format))
}

fn files(ctx: &Ctx) -> Result<&FileStore, String> {
    ctx.provided::<FileStore, _>()
        .ok_or_else(|| "this Ctx has no file store: it was built by hand".to_string())
}

/// The link to one thumbnail of `key`: `<img src>`.
#[allow(dead_code)]
pub fn src(ctx: &Ctx, key: &ObjectKey, size: &str, format: Option<&str>) -> Result<String, String> {
    let token = files(ctx)?.tokens.issue(key);
    Ok(routes::thumb_url(PATH, key, &token, spec(size, format)?))
}

/// The links to several thumbnails of `key`, each with its width:
/// `<img srcset>`. A size with no fixed width (`0xH`) is left out.
#[allow(dead_code)]
pub fn srcset(
    ctx: &Ctx,
    key: &ObjectKey,
    sizes: &[&str],
    format: Option<&str>,
) -> Result<String, String> {
    let token = files(ctx)?.tokens.issue(key);
    let specs = sizes
        .iter()
        .map(|size| spec(size, format))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(routes::srcset(PATH, key, &token, &specs))
}

#[cfg(test)]
mod tests {
    use brocanth::routing::Verb;
    use brocanth_server::Request;

    use super::*;

@CONVERT@    #[tokio::test]
    async fn a_format_or_a_size_off_the_lists_is_refused_before_the_file_is_read() {
        let app = crate::app::build().expect("the application builds");
        let cx = app.core.context();
        let files: &FileStore = cx.extract();
        // No file at this address: the lists are checked first.
        let key = ObjectKey::parse(&"ab".repeat(32)).expect("an address");
        let token = files.tokens.issue(&key);
        let link = |size: &str, format: Option<&str>| {
            routes::thumb_url(
                PATH,
                &key,
                &token,
                spec(size, format).expect("a valid spec"),
            )
        };
        // `png` is not in `formats` by default.
        let png = crate::app::handle(&app, get(&link("150x150t", Some("png")))).await;
        assert_eq!(png.status, 400);
        // A size outside `thumb_sizes` is refused before anything renders.
        let odd = crate::app::handle(&app, get(&link("151x150", Some("webp")))).await;
        assert_eq!(odd.status, 400);
    }

    #[test]
    fn srcset_gives_each_size_its_width_and_drops_a_free_one() {
        let key = ObjectKey::parse(&"ab".repeat(32)).expect("an address");
        let tokens =
            brocanth_storage::FileTokens::new(brocanth_storage::FileTokenKey::from_secret(b"test"));
        let token = tokens.issue(&key);
        let sizes = ["150x150t", "0x200", "600x600f"]
            .iter()
            .map(|size| spec(size, Some("webp")).expect("a valid spec"))
            .collect::<Vec<_>>();
        let set = routes::srcset(PATH, &key, &token, &sizes);
        assert!(set.contains("&thumb=150x150t&fmt=webp 150w, "), "{set}");
        assert!(set.ends_with("&thumb=600x600f&fmt=webp 600w"), "{set}");
        assert!(!set.contains("0x200"), "{set}");
        assert!(spec("150x150t", Some("gif")).is_err());
    }

    fn get(target: &str) -> Request {
        let (path, query) = target.split_once('?').expect("a query");
        let mut request = Request::new(Verb::Get, path);
        request.query = query.to_string();
        request
    }
}
"##;

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

const ROUTES_AGENTS_REDIRECT: &str = "\n\
    `downloads = \"redirect\"` under `[storage_routes]` answers a download with\n\
    a 307 to a signed URL of the bucket. Keep the bucket private: a public\n\
    bucket serves every object to anyone who has its address. Add the\n\
    bucket's origin to `img-src` under `[security_headers.csp.allow]`; the\n\
    start names the value when it is missing.\n";

fn routes_settings(thumbnails: bool, image: bool, cloud: bool) -> Vec<Setting> {
    let mut out = vec![
        Setting::integer(
            "max_bytes",
            2_097_152,
            "The largest file accepted, in bytes.",
        )
        .note(
            "At most `max_body_bytes` under [server] (16 MiB by default): a larger value\n\
             stops the start, because the server would refuse the upload first.",
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
    if image {
        out.push(
            Setting::list(
                "formats",
                IMAGE_FORMATS,
                "The formats &fmt= may convert a thumbnail to: webp, jpeg, png.",
            )
            .note(
                "Each format is one more rendering per size and file, so list only the ones\n\
                 the pages ask for. WebP here is lossless: right for a logo, larger than JPEG\n\
                 for a photograph.",
            ),
        );
        out.push(
            Setting::integer(
                "max_pixels",
                IMAGE_MAX_PIXELS,
                "The most pixels a source may declare before a thumbnail of it is a 413.",
            )
            .note(
                "Read from the image header before anything is decoded, so a small file that\n\
                 declares 40000x40000 costs a few bytes of reading. At most 50000000.",
            ),
        );
    }
    if cloud {
        out.push(
            Setting::text(
                "downloads",
                "proxy",
                "How a download is answered: \"proxy\" or \"redirect\".",
            )
            .note(
                "\"proxy\" sends the bytes from this process. \"redirect\" answers 307 to a URL\n\
                 the bucket signs: an image inline, anything else as an attachment. It needs a\n\
                 private bucket and the bucket's origin in the CSP img-src. HEAD and thumbnails\n\
                 still go through this process.",
            ),
        );
        out.push(
            Setting::integer(
                "signed_ttl",
                300,
                "Seconds a signed URL of a redirect lives, from 1 to 604800.",
            )
            .note(
                "It cannot be revoked, so this is the leak window. A browser keeps the redirect\n\
                 for at most half of it.",
            ),
        );
    }
    out
}

/// The formats `image` allows by default: WebP for graphics, JPEG for
/// photographs. PNG is the default rendering of an image with alpha, so it
/// needs no `&fmt=`.
const IMAGE_FORMATS: &[&str] = &["webp", "jpeg"];

/// The pixel cap `image` writes by default: a 24-megapixel camera photo.
/// Lower than the crate's hard cap, so a source the decoder could take
/// but no page needs is refused.
const IMAGE_MAX_PIXELS: i64 = 24_000_000;

fn routes_rs(project: &Project, thumbnails: bool, image: bool, cloud: bool) -> String {
    let (derive, note) = settings_derive(project, "storage_routes");
    let pick = |off: &'static str, on: &'static str| if thumbnails { on } else { off };
    let redirect = |on: &'static str, off: &'static str| if cloud { on } else { off };
    ROUTES_RS
        .replace(
            "@REDIRECT_DOC@",
            redirect(REDIRECT_DOC, ""),
        )
        .replace("@REDIRECT_USE@", redirect("Downloads, ", ""))
        .replace("@REDIRECT_FIELD@", redirect(REDIRECT_FIELD, ""))
        .replace("@REDIRECT_DEFAULT@", redirect(REDIRECT_DEFAULT, ""))
        .replace("@REDIRECT_PARSE@", redirect(REDIRECT_PARSE, ""))
        .replace("@REDIRECT_CONFIG@", redirect(REDIRECT_CONFIG, ""))
        .replace("@BUILD@", redirect(REDIRECT_BUILD, BUILD))
        .replace("@DERIVE@", &derive)
        .replace("@NOTE@", &note)
        .replace(
            "@THUMB_DOC@",
            match (thumbnails, image) {
                (_, true) => "\n//! Add `&thumb=300x200` for a thumbnail, in a size `thumb_sizes` lists, and\n//! `&fmt=webp` to convert it to a format `formats` lists.",
                (true, false) => "\n//! Add `&thumb=300x200` for a thumbnail, in a size `thumb_sizes` lists.",
                (false, false) => "",
            },
        )
        .replace(
            "@SPEC_USE@",
            if image {
                ", VariantFormat, VariantSpec"
            } else {
                pick("", ", VariantSpec")
            },
        )
        .replace(
            "@THUMB_FIELD@",
            &format!("{}{}", pick("", THUMB_FIELD), if image { IMAGE_FIELD } else { "" }),
        )
        .replace(
            "@THUMB_DEFAULT@",
            &format!("{}{}", pick("", THUMB_DEFAULT), if image { IMAGE_DEFAULT } else { "" }),
        )
        .replace(
            "@THUMB_PARSE@",
            &format!("{}{}", pick("", THUMB_PARSE), if image { IMAGE_PARSE } else { "" }),
        )
        .replace(
            "@THUMBNAILS@",
            if image {
                "thumbnails: true,\n            thumb_sizes,\n            formats,\n            max_pixels: self.settings.max_pixels,"
            } else {
                pick(
                    "thumbnails: false,",
                    "thumbnails: true,\n            thumb_sizes,",
                )
            },
        )

        .replace("@PATH@", &module_ref("storage-routes", "path"))
        .replace("@PUBLIC@", &module_ref("storage-routes", "public"))
}

const REDIRECT_DOC: &str = "
//!
//! With `downloads = \"redirect\"`, `GET /files/{address}` answers 307 to a URL
//! the bucket signs, which carries the type and the disposition inside its
//! signature: an image inline, anything else as an attachment. The redirect
//! is kept no longer than the token or half the URL's life, and sends no
//! referrer. The start signs one URL, and refuses a Content-Security-Policy
//! whose `img-src` does not allow the bucket.";

const REDIRECT_FIELD: &str = "    /// How a download is answered: \"proxy\" or \"redirect\".
    pub downloads: String,
    /// Seconds a signed URL of a redirect lives.
    pub signed_ttl: u64,
";

const REDIRECT_DEFAULT: &str = "            downloads: \"proxy\".to_string(),
            signed_ttl: 300,
";

const REDIRECT_PARSE: &str = r#"        let downloads = Downloads::parse(&self.settings.downloads).map_err(PluginError::new)?;
        // The policy the security headers send: a redirected image loads
        // only when its `img-src` allows the bucket.
        let csp = lc
            .services()
            .get::<brocanth_middleware::SecurityHeadersConfig>()
            .and_then(|headers| headers.enforced_csp());
"#;

const REDIRECT_CONFIG: &str = "
            downloads,
            signed_ttl: std::time::Duration::from_secs(self.settings.signed_ttl),
            csp,";

const BUILD: &str = "        self.routes = Some(FileRoutes::new(store, tokens, config, Guard)?);\n";

const REDIRECT_BUILD: &str = r#"        let routes = FileRoutes::new(store, tokens, config, Guard)?;
        // `serve` awaits this once before it listens: a redirecting route
        // signs one URL, so a key that cannot sign, or an `img-src` that
        // blocks the bucket, stops the start with a `help:` line.
        if routes.redirects() {
            let check = routes.clone();
            lc.ready_when(move || {
                let check = check.clone();
                async move { check.check_signing().await }
            });
        }
        self.routes = Some(routes);
"#;

const THUMB_FIELD: &str = "    /// The only sizes `?thumb=` may ask for; any other is a 400.
    pub thumb_sizes: Vec<String>,
";

const THUMB_DEFAULT: &str = "            thumb_sizes: brocanth_storage::routes::DEFAULT_THUMB_SIZES
                .iter()
                .map(|size| size.to_string())
                .collect(),
";

const IMAGE_FIELD: &str = "    /// The formats `&fmt=` may convert a thumbnail to.
    pub formats: Vec<String>,
    /// The most pixels a source may declare before a thumbnail of it is a 413.
    pub max_pixels: u64,
";

const IMAGE_DEFAULT: &str = "            formats: vec![\"webp\".to_string(), \"jpeg\".to_string()],
            max_pixels: 24_000_000,
";

const IMAGE_PARSE: &str = r#"        let formats = self
            .settings
            .formats
            .iter()
            .map(|format| {
                VariantFormat::parse(format).map_err(|e| {
                    PluginError::new(format!(
                        "[storage_routes] formats `{format}`: {e}\nhelp: list webp, jpeg or png"
                    ))
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
"#;

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
//! ```@REDIRECT_DOC@

use std::sync::Arc;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{HookRegistrar, HookSource, OnAsyncRequest, Request, RequestState};
use brocanth::plugin_order::{Order, Phase};
use brocanth_middleware::CsrfConfig;
use brocanth_storage::routes::{@REDIRECT_USE@FileRoutes, GuardFuture, RoutesConfig, UploadGuard};
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
@THUMB_FIELD@@REDIRECT_FIELD@}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_bytes: 2_097_152,
            dedupe: false,
@THUMB_DEFAULT@@REDIRECT_DEFAULT@        }
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
@THUMB_PARSE@@REDIRECT_PARSE@        let config = RoutesConfig {
            path: @PATH@.to_string(),
            max_bytes: self.settings.max_bytes,
            server_max_body: Some(crate::plugins::max_body().map_err(PluginError::new)?),
            public: @PUBLIC@,
            @THUMBNAILS@
            dedupe: self.settings.dedupe,
            csrf_field: csrf.field.clone(),
            csrf_header,@REDIRECT_CONFIG@
            ..RoutesConfig::default()
        };
@BUILD@        Ok(())
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
