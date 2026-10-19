//! `policy` -- what `brocanth create` writes when somebody asks for rules.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that a comparison takes a [`Column`] and
//! never a string, that [`Bound::or_deny`] is a word somebody wrote down,
//! that [`Entity::TABLE`] is an [`Ident`] checked at compile time, and that
//! the two spellings of [`Dialect`] compile the same meaning rather than the
//! same text. Those move when this crate moves, and a generator living in
//! another repository would learn about it from a bug report.
//!
//! [`Column`]: crate::Column
//! [`Bound::or_deny`]: crate::Bound::or_deny
//! [`Entity::TABLE`]: crate::Entity::TABLE
//! [`Ident`]: crate::Ident
//! [`Dialect`]: crate::Dialect
//!
//! # Why there is a plugin at all
//!
//! This crate has none of its own, and it should not: it opens nothing, holds
//! nothing and never sees a request. The generated plugin exists for one
//! reason, and it is a small one said plainly. Every compile entry point takes
//! a [`Dialect`](crate::Dialect) argument, deliberately, so a call site says
//! which one it meant -- but an application has one database, and twenty call
//! sites naming it separately is nineteen chances to name the wrong one. So
//! the dialect is read once, checked once at startup where a typo can still be
//! answered, and handed to every request on the context. The call site still
//! passes it; it just has one place to get it from.
//!
//! It also makes the example reachable. A `src/plugins/policy/` that nothing
//! calls is a directory of dead-code warnings on a project's first build, and
//! a first build that warns is a bad first minute.
//!
//! # Why it does not need `model`
//!
//! It was the obvious requirement and it is the wrong one. A policy compiles
//! to a `WHERE` fragment and an ordered bind list, which does suggest that a
//! project with no ORM and no SQL driver has nothing to run it against -- but
//! two of this crate's three interpretations need no database at all.
//! `eval()` answers over a row the caller already holds, and `audience()`
//! inverts a predicate into a broadcast key, which is the whole reason
//! `brocanth-realtime` depends on this crate. Requiring `model` would refuse a
//! project that broadcasts and stores nothing, to protect it from a
//! `Condition` it would simply not compile. So: `after("model")`, for the
//! order of the two config sections, and nothing stronger.
//!
//! That ordering is load-bearing in one place. The generated `[policy]`
//! dialect follows whatever `[model]` was given: the model module's own
//! default URL is SQLite, so a project with both gets `dialect = "sqlite"` and
//! a project with neither gets Postgres, which is this crate's default and
//! brocanth's base. A condition compiled for the wrong dialect is valid SQL
//! the engine then rejects, and the generated file says so next to the field.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{plugin_line, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// Authorisation, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
pub struct Policy;

impl Module for Policy {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "policy",
            "typed predicates over an entity's columns; no ORM, no driver, no core",
        )
        .krate(Dep::brocanth("brocanth-policy"))
        // Only for the order of the two sections and the two builder lines:
        // `[policy] dialect` has to say what `[model] url` says, so it reads
        // under it. Neither module requires the other -- see the module
        // documentation for why this is not `needs`.
        .after("config")
        .after("model")
        .absent(
            "brocanth-policy",
            "no rule algebra; authorisation is whatever each handler remembered to check",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let dialect = dialect(project);
        s.file("src/plugins/policy/mod.rs", plugin_rs(project, dialect));
        s.at(Slot::PluginsMod).line("pub mod policy;");
        s.at(Slot::AppPlugins)
            .block(plugin_line(project, "policy", "Policies"));
        // The dialect is read at startup, so a brand-new project uses the
        // value it was given rather than warning that nothing does.
        s.at(Slot::AppContext).block(
            "    let rules: &crate::plugins::policy::Rules = cx.extract();\n\
            \x20   eprintln!(\"policy: compiling for {:?}\", rules.dialect);"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
        if project.has("config") {
            s.at(Slot::ConfigSections).block(section(project, dialect));
        }
    }
}

/// Which dialect this project's conditions are compiled for.
///
/// The model module's own default URL is SQLite, so a project that has one
/// gets SQLite here and the two sections agree out of the box. A project with
/// no model gets Postgres, which is this crate's default and brocanth's base.
fn dialect(project: &Project) -> &'static str {
    if project.has("model") {
        "sqlite"
    } else {
        "postgres"
    }
}

/// The `[policy]` section of `config/development.toml`.
fn section(project: &Project, dialect: &str) -> String {
    let agrees = if project.has("model") {
        "# This follows `[model] url`, which the scaffold pointed at SQLite. Move the\n\
         # two together: `IS NOT DISTINCT FROM` and `IS NOT` mean the same thing and\n\
         # only one of them parses, so a condition compiled for the wrong dialect is\n\
         # valid SQL that the engine rejects at the worst possible moment.\n"
    } else {
        "# postgres or sqlite. Nothing in this project opens a database yet, so this\n\
         # is the dialect your conditions are compiled for and nothing else. Add the\n\
         # model later and these two have to agree: `IS NOT DISTINCT FROM` and\n\
         # `IS NOT` mean the same thing and only one of them parses.\n"
    };
    format!("\n[policy]\n{agrees}dialect = \"{dialect}\"\n")
}

fn plugin_rs(project: &Project, dialect: &str) -> String {
    let (derive, note) = settings_derive(project, "policy");
    // One extra paragraph when the broker is here, because this is the pair
    // §10 is about and a reader with both should be told where the join is.
    let realtime = if project.has("realtime") {
        "//!\n\
         //! # This project also has the broker\n\
         //!\n\
         //! `Pred::audience` is the join, and it is why `brocanth-realtime`\n\
         //! depends on this crate. A predicate you can inspect inverts into an\n\
         //! audience -- `author_id = 7` becomes the key `(author_id, 7)` -- so the\n\
         //! broker files a subscriber under that key and one write is a map lookup\n\
         //! rather than one query per subscriber. A rule that is a string can only\n\
         //! be executed, which is why PocketBase runs the query.\n"
    } else {
        ""
    };
    format!(
        r##"//! Authorisation, as this application writes it.
//!
//! A rule is a predicate and a check is a witness. `PostPolicy` below is the
//! first; `Verified` is the second; the two are not alternatives.
//!
//! `brocanth-policy` is in `Cargo.toml` because this file is here, and through
//! no other route. Delete the registration in `src/app.rs` and delete this
//! file, and the application has no rule algebra and links nothing to have one
//! with.
//!
//! # The bug this file cannot have
//!
//! PocketBase validates a rule against an empty request object, so a
//! misspelled field in a negated rule becomes `'' IS NOT 'banned'`, which is
//! true, which grants access -- silently, for ever. Two things here make that
//! impossible, and both are visible below:
//!
//! 1. **A field name is not a string.** `Pred::eq` takes a `PostColumn`, so
//!    `PostColumn::Autor` is a compile error. There is no
//!    `Pred::eq("author_id", ..)` to fall back to and no string-to-filter
//!    parser anywhere in this framework.
//! 2. **An absent value is a word somebody wrote.** An `Option` does not turn
//!    into a `Bound`. The author writes `Bound::or_deny`, `or_allow` or
//!    `or_null`, and the word is in the diff for a reviewer to disagree with.
//!
//! # Where the rules go
//!
//! `Post` is here so that this file compiles into something that runs. Move
//! it, and whatever follows it, into `src/policies/` once there is more than
//! one -- what must stay here is the plugin, because that is what reads the
//! dialect and puts it on the context.
//!
//! The column enum is written by hand on purpose: the `policy!` macro is
//! deliberately late, after a real application has used the algebra, so that
//! the syntax is informed rather than imitated.
//!
//! # Listing, and then writing
//!
//! A list is a `WHERE` fragment, so the paginator keeps its totals:
//!
//! ```ignore
//! let rules: &Rules = cx.extract();
//! let condition = PostPolicy.list(&visitor).condition::<Post>(rules.dialect)?;
//! // "SELECT .. FROM posts WHERE " + condition.sql(), bound to condition.binds()
//! ```
//!
//! A write is the same predicate carried into the statement, which is what
//! closes the window between the check and the change:
//!
//! ```ignore
//! let admission = PostPolicy.update(&visitor).admission::<Post>(rules.dialect, id)?;
//! let rows = /* run admission.query(), count the rows */;
//! let verified = admission.resolve(rows)?;          // zero rows is a denial
//! let write = verified.update(rules.dialect, [(PostColumn::Status, "published".into())])?;
//! // run write.query(); then:
//! write.applied(affected)?;                          // zero affected is RaceLost
//! ```
//!
//! `Verified` is taken by value everywhere it is used, and a debug build fails
//! the test that checked one and then forgot to spend it.
{realtime}
use brocanth::plugin_api::{{Lifecycle, Plugin, PluginError}};
use brocanth::plugin_context::{{ContextBuilder, ContextItem, ProvidesContext}};
use brocanth_policy::{{Bound, Column, Dialect, Entity, EntityPred, FieldMask, Ident, Policy, Pred}};

// ---------------------------------------------------------------------------
// An entity
// ---------------------------------------------------------------------------

/// The columns of `posts`, as an enum, because a rule is written in them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PostColumn {{
    /// Primary key.
    Id,
    /// Who wrote it.
    AuthorId,
    /// `draft` or `published`.
    Status,
}}

impl Column for PostColumn {{
    /// Required rather than derived: a changed-field name that is not one of
    /// these is rejected instead of quietly permitted.
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status];

    fn name(&self) -> &'static str {{
        match self {{
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
        }}
    }}
}}

/// The table the rules below are about.
pub struct Post;

impl Entity for Post {{
    type Column = PostColumn;
    /// Checked at compile time, because a table name is program text and the
    /// only honest moment to reject one is while the program is being built.
    const TABLE: Ident = Ident::must("posts");
    const PRIMARY_KEY: Self::Column = PostColumn::Id;
}}

// ---------------------------------------------------------------------------
// A subject, and the rules about it
// ---------------------------------------------------------------------------

/// Whoever is asking. `brocanth-policy` never looks inside it.
///
/// An anonymous visitor is this application's own `None`, turned into a
/// decision at the point a rule reads it.
#[derive(Clone, Debug, Default)]
pub struct Visitor {{
    /// The signed-in account, when there is one.
    pub account_id: Option<i64>,
    /// Whether this one may see everything.
    pub admin: bool,
}}

impl Visitor {{
    /// Nobody. What a request with no session carries.
    pub fn anonymous() -> Self {{
        Self::default()
    }}
}}

/// The rules for `posts`.
///
/// Only `list` is required, because list authorisation is the one that cannot
/// be done per object: a paginator that filters after reading lies about its
/// totals. Everything else defaults to deny, which is what makes shipping this
/// file as it stands safe rather than lucky.
#[derive(Clone, Copy, Debug, Default)]
pub struct PostPolicy;

impl Policy for PostPolicy {{
    type Entity = Post;
    type Subject = Visitor;

    fn list(&self, who: &Visitor) -> EntityPred<Post> {{
        if who.admin {{
            // `Pred::is_always` reports this, so a test can assert which
            // subjects see everything.
            return Pred::Always;
        }}
        Pred::any([
            Pred::eq(PostColumn::Status, "published"),
            // `or_deny` is the word that says what an anonymous visitor's
            // missing id means: they own nothing.
            Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.account_id)),
        ])
    }}

    fn create(&self, who: &Visitor) -> EntityPred<Post> {{
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.account_id))
    }}

    fn update(&self, who: &Visitor) -> EntityPred<Post> {{
        if who.admin {{
            return Pred::Always;
        }}
        Pred::eq(PostColumn::AuthorId, Bound::or_deny(who.account_id))
    }}

    fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {{
        if who.admin {{
            FieldMask::except([PostColumn::Id])
        }} else {{
            // The author edits the body, never the status. The mask is the
            // other half of the rule: `update` says which rows, this says
            // which columns.
            FieldMask::only([PostColumn::AuthorId])
        }}
    }}
}}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// What this plugin is built from.
///
/// {note}
{derive}
pub struct Settings {{
    /// `postgres` or `sqlite`. A string here and an enum below: a config file
    /// is text, and the translation is checked at startup where a typo can
    /// still be answered.
    pub dialect: String,
}}

impl Default for Settings {{
    fn default() -> Self {{
        Self {{
            dialect: "{dialect}".to_string(),
        }}
    }}
}}

/// What this plugin puts on the context.
///
/// The dialect, and only the dialect. Every compile entry point still takes
/// one as an argument -- the call site says which it meant -- and this is
/// where the call site gets it, so one database is named once.
///
/// The policies are not here. `PostPolicy` is a unit struct and a value, not
/// a service: a handler writes `PostPolicy.list(&who)` and has nothing to
/// look up. What a handler cannot work out on its own is which dialect this
/// deployment is, so that is what the context carries.
pub struct Rules {{
    /// Which dialect this application's conditions are compiled for.
    pub dialect: Dialect,
}}

impl ContextItem for Rules {{}}

pub struct Policies {{
    settings: Settings,
    /// Read at `configure` and not in `new`, because turning a string into a
    /// dialect can fail and `Plugin::new` cannot.
    dialect: Option<Dialect>,
}}

impl Plugin for Policies {{
    type Config = Settings;
    type Provides = ProvidesContext<Rules>;
    const NAME: &'static str = "policy";

    fn new(config: Settings) -> Self {{
        Self {{
            settings: config,
            dialect: None,
        }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        let dialect = match self.settings.dialect.as_str() {{
            "postgres" => Dialect::Postgres,
            "sqlite" => Dialect::Sqlite,
            other => {{
                return Err(PluginError::new(format!(
                    "[policy] dialect is {{other:?}}; write postgres or sqlite"
                )))
            }}
        }};

        // Compile one rule at startup. It costs a string and it answers the
        // only question this file can get wrong without anybody noticing: a
        // column whose name is not a bare identifier fails the query rather
        // than escaping into it, and finding that out here is better than
        // finding it out on the first list request.
        let condition = PostPolicy
            .list(&Visitor::anonymous())
            .condition::<Post>(dialect)
            .map_err(|e| PluginError::new(format!("[policy] posts: {{e}}")))?;
        eprintln!("[policy] posts, anonymous: {{}}", condition.sql());

        self.dialect = Some(dialect);
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the dialect is here by the time any request builds a context.
        if let Some(dialect) = self.dialect {{
            cx.insert(Rules {{ dialect }});
        }}
    }}
}}
"##
    )
}
