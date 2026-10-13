//! The value model, and the provenance that every node carries.
//!
//! # Why the model is ours and not a format's
//!
//! The crate used to keep its document as `toml::Table`. That was correct while
//! TOML was the only format, and it stops being correct the moment a second one
//! arrives: a `toml::Table` cannot be built without linking a TOML parser, and
//! PLAN.md §8.2 says *"a feature that is off must mean the code is absent"*. A
//! service configured from JSON and the environment must not link a TOML
//! parser, so no format owns the model.
//!
//! # Provenance is in the model, not beside it
//!
//! Every [`Value`] holds the [`Provenance`] of the value it carries, and the
//! provenance of every value it replaced. This is the point of the crate, and
//! it is why the field is not an `Option` and not a side table: a value that
//! cannot say where it came from cannot exist here.
//!
//! PLAN.md §8 allows Rails' defaults only on that condition: *"'Convention over
//! configuration' is Rails' good idea; 'the framework decided for you and you
//! cannot see where' is its bad one, and the line between them is whether the
//! decision is written in your `main`."* A plugin's default is not written in
//! `main`. What makes it legitimate anyway is that
//! [`Effective::dump`](crate::Effective::dump) prints it, attributed, next to
//! every value that beat it.
//!
//! # Open and closed tables
//!
//! A [`Table`] is *closed* when its key set is complete and *open* when any key
//! is allowed. The distinction is not guessed: serde itself makes it, because a
//! Rust struct serialises through `serialize_struct` and a map through
//! `serialize_map`. [`crate::ser`] records which one it saw. That is how the
//! crate knows, with no schema and no declaration, whether `[logger] levle =
//! "warn"` is a typo or a legitimate entry in a `HashMap`.

use std::collections::BTreeMap;
use std::fmt;
#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use crate::config::Origin;
use crate::config::Source;

// ---------------------------------------------------------------------------
// Formats
// ---------------------------------------------------------------------------

/// A file format this crate can read.
///
/// Each one is a cargo feature, and a feature that is off leaves its parser out
/// of the build. The variant stays, so an error about a `.yaml` file can say
/// *which* feature to turn on rather than "unknown extension".
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Format {
    /// TOML, through the `toml` feature. On by default.
    Toml,
    /// JSON, through the `json` feature.
    Json,
    /// YAML, through the `yaml` feature.
    Yaml,
}

/// Every format, in the order a slot is probed.
pub(crate) const ALL_FORMATS: &[Format] = &[Format::Toml, Format::Json, Format::Yaml];

impl Format {
    /// The feature name, which is also the format's name in an error.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Format::Toml => "toml",
            Format::Json => "json",
            Format::Yaml => "yaml",
        }
    }

    /// The file extensions that select this format, preferred spelling first.
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Toml => &["toml"],
            Format::Json => &["json"],
            Format::Yaml => &["yaml", "yml"],
        }
    }

    /// The format an extension selects, whether or not its feature is on.
    ///
    /// Answering for a disabled format on purpose: the error an operator needs
    /// is "`.yaml` needs the `yaml` feature", not "unknown extension".
    #[must_use]
    pub fn from_extension(ext: &str) -> Option<Self> {
        let lower = ext.to_ascii_lowercase();
        ALL_FORMATS
            .iter()
            .copied()
            .find(|f| f.extensions().contains(&lower.as_str()))
    }

    /// Whether this build contains the parser.
    #[must_use]
    pub const fn is_enabled(self) -> bool {
        match self {
            Format::Toml => cfg!(feature = "toml"),
            Format::Json => cfg!(feature = "json"),
            Format::Yaml => cfg!(feature = "yaml"),
        }
    }

    /// The formats this build can read, in probe order.
    #[must_use]
    pub fn enabled() -> Vec<Format> {
        ALL_FORMATS
            .iter()
            .copied()
            .filter(|f| f.is_enabled())
            .collect()
    }

    /// Every extension this build can read, in probe order.
    #[must_use]
    pub fn enabled_extensions() -> Vec<&'static str> {
        Self::enabled()
            .into_iter()
            .flat_map(|f| f.extensions().iter().copied())
            .collect()
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Where one value came from.
///
/// Four origins, and they are exhaustive: a value in a loaded configuration is
/// a plugin's default, a line in a file, an environment variable, or a document
/// the application embedded in its own source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// A registered plugin's own default. The framework decided, and this says
    /// which plugin decided.
    Default {
        /// The section name the plugin was registered under.
        plugin: Arc<str>,
    },
    /// A line in a file.
    File {
        /// Which file, and which of the four slots it occupies.
        file: Arc<Source>,
        /// 1-indexed line.
        line: u32,
        /// 1-indexed column.
        column: u32,
    },
    /// An environment variable, named in full.
    EnvVar {
        /// The variable's name, as spelled in the environment.
        var: Arc<str>,
    },
    /// A document the application passed in, rather than one found on disk.
    Inline {
        /// What to call it in a dump.
        label: Arc<str>,
    },
}

impl Provenance {
    /// A default contributed by the plugin registered as `plugin`.
    #[must_use]
    pub fn default_of(plugin: &str) -> Self {
        Provenance::Default {
            plugin: Arc::from(plugin),
        }
    }

    /// An environment variable.
    #[must_use]
    pub fn env(var: &str) -> Self {
        Provenance::EnvVar {
            var: Arc::from(var),
        }
    }

    /// An embedded document.
    #[must_use]
    pub fn inline(label: &str) -> Self {
        Provenance::Inline {
            label: Arc::from(label),
        }
    }

    /// `true` when the value is a plugin's default, untouched by any file or
    /// variable. The one question a dump must answer at a glance.
    #[must_use]
    pub fn is_default(&self) -> bool {
        matches!(self, Provenance::Default { .. })
    }

    /// The file this came from, if it came from one.
    #[must_use]
    pub fn file(&self) -> Option<&Arc<Source>> {
        match self {
            Provenance::File { file, .. } => Some(file),
            _ => None,
        }
    }
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provenance::Default { plugin } => write!(f, "default ({plugin})"),
            Provenance::File { file, line, .. } => {
                write!(f, "{}:{line}", file.path.display())
            }
            Provenance::EnvVar { var } => write!(f, "${var}"),
            Provenance::Inline { label } => write!(f, "<{label}>"),
        }
    }
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// A table: named entries, and whether the key set is complete.
///
/// Entries are kept in name order, so a dump, an error and a merge are all
/// deterministic whatever order the file wrote them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Table {
    entries: BTreeMap<String, Value>,
    open: bool,
}

impl Table {
    /// An empty table whose key set is complete.
    #[must_use]
    pub fn closed() -> Self {
        Self {
            entries: BTreeMap::new(),
            open: false,
        }
    }

    /// An empty table that accepts any key. What a `HashMap` field becomes, and
    /// what a parsed file produces, because a file cannot declare a key set.
    #[must_use]
    pub fn open() -> Self {
        Self {
            entries: BTreeMap::new(),
            open: true,
        }
    }

    /// Whether any key is allowed here.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// One entry.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.get(key)
    }

    /// One entry, mutably.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.get_mut(key)
    }

    /// Adds or replaces an entry, returning what was there.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        self.entries.insert(key.into(), value)
    }

    /// Removes an entry.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.entries.remove(key)
    }

    /// Whether the key is present.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Entries, in name order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter()
    }

    /// Key names, in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// How many entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether there are no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl IntoIterator for Table {
    type Item = (String, Value);
    type IntoIter = std::collections::btree_map::IntoIter<String, Value>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl std::ops::Index<&str> for Table {
    type Output = Value;

    fn index(&self, key: &str) -> &Value {
        self.entries
            .get(key)
            .unwrap_or_else(|| panic!("no key `{key}` in this table"))
    }
}

/// What a value is.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueKind {
    /// Text.
    String(String),
    /// A whole number.
    Integer(i64),
    /// A number with a fractional part.
    Float(f64),
    /// `true` or `false`.
    Boolean(bool),
    /// A TOML date, time or date-time, kept as written.
    ///
    /// JSON and YAML have no such type, so a configuration that must stay
    /// portable across the three formats writes a string. Deserialising one
    /// gives a string, which is what every date crate's serde support reads.
    Datetime(String),
    /// A sequence.
    Array(Vec<Value>),
    /// Named entries.
    Table(Table),
    /// Explicitly no value.
    ///
    /// JSON and YAML `null`, and `Option::None` in a plugin's defaults. It
    /// never survives a merge: in a layer it *removes* the key, so a deployment
    /// can write `level: null` to drop back to the plugin's default rather than
    /// having to know what that default is.
    Unset,
}

impl ValueKind {
    /// The kind's name, for an error message.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            ValueKind::String(_) => "string",
            ValueKind::Integer(_) => "integer",
            ValueKind::Float(_) => "float",
            ValueKind::Boolean(_) => "boolean",
            ValueKind::Datetime(_) => "datetime",
            ValueKind::Array(_) => "array",
            ValueKind::Table(_) => "table",
            ValueKind::Unset => "unset",
        }
    }
}

/// One configuration value, and where it came from.
#[derive(Clone, Debug)]
pub struct Value {
    kind: ValueKind,
    from: Provenance,
    shadowed: Vec<Value>,
}

impl PartialEq for Value {
    /// Compares what the value *is*. Two values are equal when they say the
    /// same thing, whatever file they were read from — which is what a
    /// round-trip test across three formats needs to assert.
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Value {
    /// A value with a provenance.
    #[must_use]
    pub fn new(kind: ValueKind, from: Provenance) -> Self {
        Self {
            kind,
            from,
            shadowed: Vec::new(),
        }
    }

    /// Where this value came from.
    #[must_use]
    pub fn provenance(&self) -> &Provenance {
        &self.from
    }

    /// Every value this one overrode, nearest layer first.
    ///
    /// The "what was the port before, and who set it" answer, and the reason
    /// the dump can print `was 3000  ./config/production.yaml:2` under an
    /// overridden key. Each entry carries its own value and its own
    /// provenance, and has an empty history of its own: the chain is flat, so
    /// a key overridden four times lists four entries rather than nesting
    /// four deep.
    #[must_use]
    pub fn shadowed(&self) -> &[Value] {
        &self.shadowed
    }

    /// What this value is.
    #[must_use]
    pub fn kind(&self) -> &ValueKind {
        &self.kind
    }

    /// Consumes the value and returns what it is.
    #[must_use]
    pub fn into_kind(self) -> ValueKind {
        self.kind
    }

    /// The text, if this is a string.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match &self.kind {
            ValueKind::String(s) | ValueKind::Datetime(s) => Some(s),
            _ => None,
        }
    }

    /// The number, if this is an integer.
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self.kind {
            ValueKind::Integer(i) => Some(i),
            _ => None,
        }
    }

    /// The number, if this is a float.
    #[must_use]
    pub fn as_float(&self) -> Option<f64> {
        match self.kind {
            ValueKind::Float(f) => Some(f),
            _ => None,
        }
    }

    /// The flag, if this is a boolean.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self.kind {
            ValueKind::Boolean(b) => Some(b),
            _ => None,
        }
    }

    /// The elements, if this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&[Value]> {
        match &self.kind {
            ValueKind::Array(a) => Some(a),
            _ => None,
        }
    }

    /// The entries, if this is a table.
    #[must_use]
    pub fn as_table(&self) -> Option<&Table> {
        match &self.kind {
            ValueKind::Table(t) => Some(t),
            _ => None,
        }
    }

    /// The entries, if this is a table.
    pub fn as_table_mut(&mut self) -> Option<&mut Table> {
        match &mut self.kind {
            ValueKind::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Whether this is a table.
    #[must_use]
    pub fn is_table(&self) -> bool {
        matches!(self.kind, ValueKind::Table(_))
    }

    /// Whether this is the explicit absence of a value.
    #[must_use]
    pub fn is_unset(&self) -> bool {
        matches!(self.kind, ValueKind::Unset)
    }

    /// The value at a dotted path, relative to this one.
    ///
    /// `cfg.root().at("server.pool.max_size")`. A segment that names an array
    /// index works too: `at("db.hosts.0")`.
    #[must_use]
    pub fn at(&self, path: &str) -> Option<&Value> {
        let mut here = self;
        for segment in path.split('.').filter(|s| !s.is_empty()) {
            here = match &here.kind {
                ValueKind::Table(t) => t.get(segment)?,
                ValueKind::Array(a) => a.get(segment.parse::<usize>().ok()?)?,
                _ => return None,
            };
        }
        Some(here)
    }

    /// A one-line rendering for a dump: `"warn"`, `8080`, `["a", "b"]`.
    #[must_use]
    pub fn display(&self) -> String {
        match &self.kind {
            ValueKind::String(s) => format!("{s:?}"),
            ValueKind::Integer(i) => i.to_string(),
            ValueKind::Float(f) => f.to_string(),
            ValueKind::Boolean(b) => b.to_string(),
            ValueKind::Datetime(s) => s.clone(),
            ValueKind::Array(a) => {
                let parts: Vec<String> = a.iter().map(Value::display).collect();
                format!("[{}]", parts.join(", "))
            }
            ValueKind::Table(t) => format!("{{ {} keys }}", t.len()),
            ValueKind::Unset => "(unset)".to_string(),
        }
    }

    /// Every leaf under this value, as a dotted path and the value itself.
    ///
    /// An array is one leaf, not one per element, because the merge rule
    /// replaces an array whole. A dump that listed `hosts.0`, `hosts.1` and
    /// `hosts.2` separately would suggest they can be set separately, and in a
    /// file they cannot.
    #[must_use]
    pub fn leaves(&self) -> Vec<(String, &Value)> {
        let mut out = Vec::new();
        self.collect_leaves(String::new(), &mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, prefix: String, out: &mut Vec<(String, &'a Value)>) {
        match &self.kind {
            ValueKind::Table(t) if !t.is_empty() => {
                for (key, value) in t.iter() {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    value.collect_leaves(path, out);
                }
            }
            _ => out.push((prefix, self)),
        }
    }
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Deep merge: `over` wins.
///
/// **Tables recurse; scalars and arrays are replaced whole.** The rule the
/// crate already had, unchanged, and it survives contact with both of the new
/// sources:
///
/// * **Plugin defaults.** Recursion is exactly what makes a plugin's defaults
///   useful: a file that sets `[server] port` keeps the plugin's `host`,
///   `workers` and `pool.timeout`. Replacing a table whole would mean a file
///   that mentions one key silently discards every other default in that
///   section, which is the Rails failure this crate exists to avoid.
/// * **Environment variables.** The env layer builds one table from every
///   matching variable *before* it is merged, so `MY_APP_DB__HOSTS__0` and
///   `MY_APP_DB__HOSTS__1` build one two-element array and that array then
///   replaces whatever was below it whole. Setting index 0 alone over a
///   three-element default gives a one-element array, not a patched one. That
///   is the rule holding, not an exception to it, and it is tested.
///
/// A value of kind [`ValueKind::Unset`] in `over` *removes* the key instead of
/// replacing it.
pub fn merge(base: &mut Value, over: Value) {
    match (&mut base.kind, over.kind) {
        (ValueKind::Table(base_t), ValueKind::Table(over_t)) => {
            merge_tables(base_t, over_t);
        }
        (_, kind) => {
            // Nearest first: whatever `over` had already beaten, then the value
            // standing here, then whatever that one had beaten. `over`'s own
            // history must not be dropped — that is how a defaults layer slid
            // underneath a merged document keeps the file-by-file chain.
            let mut history = over.shadowed;
            history.push(Value {
                kind: std::mem::replace(&mut base.kind, kind),
                from: std::mem::replace(&mut base.from, over.from),
                shadowed: Vec::new(),
            });
            history.append(&mut base.shadowed);
            base.shadowed = history;
        }
    }
}

/// [`merge`], for the two tables themselves.
pub fn merge_tables(base: &mut Table, over: Table) {
    for (key, value) in over {
        if value.is_unset() {
            base.remove(&key);
            continue;
        }
        match base.get_mut(&key) {
            Some(existing) => merge(existing, value),
            None => {
                base.insert(key, value);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Line and column from a byte offset
// ---------------------------------------------------------------------------

/// Turns a byte offset into a 1-indexed line and column.
///
/// Two of the three parsers report a byte range rather than a position, so this
/// is the one place that converts. Columns count characters, not bytes, because
/// a column number is for a human looking at a line in an editor.
#[cfg_attr(
    not(any(feature = "toml", feature = "json", feature = "yaml")),
    allow(dead_code)
)]
pub(crate) fn line_and_column(text: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(text.len());
    let before = &text[..offset];
    let line = before.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let column = text[line_start..offset].chars().count() + 1;
    (line as u32, column as u32)
}

/// A file source, for a provenance that points into one. Tests build these;
/// the loader builds its own from the slot it probed.
#[cfg(test)]
pub(crate) fn file_source(path: PathBuf, origin: Origin, format: Format) -> Arc<Source> {
    Arc::new(Source {
        path,
        origin,
        format,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(kind: ValueKind) -> Value {
        Value::new(kind, Provenance::inline("test"))
    }

    #[test]
    fn line_and_column_are_one_indexed() {
        let text = "a = 1\nb = 2\nc = 3\n";
        assert_eq!(line_and_column(text, 0), (1, 1));
        assert_eq!(line_and_column(text, 6), (2, 1));
        assert_eq!(line_and_column(text, 10), (2, 5));
        assert_eq!(line_and_column(text, 12), (3, 1));
    }

    #[test]
    fn a_column_counts_characters_not_bytes() {
        let text = "clé = \"ok\"";
        // `é` is two bytes; the `=` is the fifth character.
        let (line, column) = line_and_column(text, text.find('=').unwrap());
        assert_eq!((line, column), (1, 5));
    }

    #[test]
    fn merging_recurses_into_tables_and_replaces_everything_else() {
        let mut base = Table::open();
        let mut pool = Table::open();
        pool.insert("size", v(ValueKind::Integer(5)));
        pool.insert("timeout", v(ValueKind::Integer(30)));
        base.insert("pool", v(ValueKind::Table(pool)));
        base.insert(
            "hosts",
            v(ValueKind::Array(vec![
                v(ValueKind::String("a".into())),
                v(ValueKind::String("b".into())),
            ])),
        );

        let mut over = Table::open();
        let mut over_pool = Table::open();
        over_pool.insert("size", v(ValueKind::Integer(20)));
        over.insert("pool", v(ValueKind::Table(over_pool)));
        over.insert(
            "hosts",
            v(ValueKind::Array(vec![v(ValueKind::String("z".into()))])),
        );

        merge_tables(&mut base, over);

        let pool = base["pool"].as_table().unwrap();
        assert_eq!(pool["size"].as_integer(), Some(20));
        assert_eq!(
            pool["timeout"].as_integer(),
            Some(30),
            "nested key survives"
        );
        assert_eq!(base["hosts"].as_array().unwrap().len(), 1, "array replaced");
    }

    #[test]
    fn an_unset_in_the_higher_layer_removes_the_key() {
        let mut base = Table::open();
        base.insert("level", v(ValueKind::String("info".into())));
        let mut over = Table::open();
        over.insert("level", v(ValueKind::Unset));
        merge_tables(&mut base, over);
        assert!(!base.contains_key("level"));
    }

    #[test]
    fn a_replaced_value_remembers_what_it_replaced() {
        let mut base = v(ValueKind::Integer(3000));
        let over = Value::new(ValueKind::Integer(8080), Provenance::env("MY_APP_X__Y"));
        merge(&mut base, over);
        assert_eq!(base.as_integer(), Some(8080));
        assert_eq!(base.shadowed().len(), 1);
        assert_eq!(base.shadowed()[0].as_integer(), Some(3000), "the old value");
        assert!(matches!(
            base.shadowed()[0].provenance(),
            Provenance::Inline { .. }
        ));
    }

    #[test]
    fn an_array_is_one_leaf_because_an_array_is_replaced_whole() {
        let mut root = Table::open();
        let mut db = Table::open();
        db.insert(
            "hosts",
            v(ValueKind::Array(vec![v(ValueKind::String("a".into()))])),
        );
        root.insert("db", v(ValueKind::Table(db)));
        let root = v(ValueKind::Table(root));

        let paths: Vec<String> = root.leaves().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["db.hosts".to_string()]);
    }

    #[test]
    fn an_extension_names_its_format_even_when_the_feature_is_off() {
        assert_eq!(Format::from_extension("yml"), Some(Format::Yaml));
        assert_eq!(Format::from_extension("YAML"), Some(Format::Yaml));
        assert_eq!(Format::from_extension("ini"), None);
    }

    #[test]
    #[cfg(feature = "toml")]
    fn toml_is_always_enabled_by_default_and_is_probed_first() {
        assert_eq!(Format::enabled().first(), Some(&Format::Toml));
    }
}
