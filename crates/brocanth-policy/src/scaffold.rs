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
//! The generated `[policy]` dialect is `postgres`, with a model or without.
//! Brocanth supports PostgreSQL only (2026-11-11), so the setting has one
//! legal value. It stays so that the generated `Rules` keeps its shape.
//!
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::snippet::{configurable, settings_derive, PLUGIN_TRAIT_USE};
use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Setting, Slot};

/// Authorisation, as a choice on the `brocanth create` menu.
///
/// A unit struct, because a module is asked once per run and everything it
/// needs arrives in [`Project`].
///
/// Named `Authorisation` and not `Policy`, which is this crate's own trait.
/// A second item under that name in the crate makes rustc print the full
/// path in every error that names the trait, and a UI test in
/// `brocanth-resource` pins that error. A crate whose tests take the CLI
/// compiles this module, so the name must not collide.
pub struct Authorisation;

impl Module for Authorisation {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "policy",
            "typed predicates over an entity's columns; no ORM, no driver, no core",
        )
        .category(Category::Middleware)
        .adapts_to(
            "config",
            "its settings are read from the config file, not written as a literal",
        )
        .adapts_to(
            "model",
            "the dialect note names the database the model reads",
        )
        .adapts_to(
            "realtime",
            "the docs say where a rule joins a broadcast key",
        )
        .asked_in(
            "doors",
            "Authorisation rules that compile into the WHERE clause",
        )
        .krate(Dep::brocanth("brocanth-policy"))
        // Only for the order of the two sections and the two builder lines:
        // `[policy] dialect` has to say what `[db] url` says, so it reads
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
        let dialect = DIALECT;
        s.file("src/plugins/policy/mod.rs", plugin_rs(project, dialect));
        s.at(Slot::PluginsMod).line("pub mod policy;");
        configurable(s, project, "policy", "Policies", settings(project, dialect));
        // The dialect is read at startup, so a brand-new project uses the
        // value it was given rather than warning that nothing does.
        s.at(Slot::AppContext).block(
            "    let rules: &crate::plugins::policy::Rules = cx.extract();\n\
            \x20   let dialect = &rules.dialect;\n\
            \x20   crate::plugins::say!(info, \"policy: compiling for {dialect:?}\"; ?dialect);"
                .to_string(),
        );
        if !project.has("config") {
            // The literal form calls `Plugin::new`, which is a trait
            // method, so the trait has to be in scope. Every plugin
            // module needs the same line, and `use` twice does not
            // compile.
            s.at(Slot::AppUses).once(PLUGIN_TRAIT_USE);
        }
    }
}

/// Which dialect this project's conditions are compiled for.
///
/// Postgres, with a model or without. The model runs over the `db` pool,
/// which is Postgres, so the two sections agree out of the box. Without a
/// model it is this crate's default and brocanth's base.
const DIALECT: &str = "postgres";

/// The `[policy]` section of `config/development.toml`.
fn settings(project: &Project, dialect: &str) -> Vec<Setting> {
    let agrees = if project.has("model") {
        "This follows `[db] url`, the Postgres database the model reads.\n\
         Brocanth supports Postgres only, so `postgres` is the one legal value."
    } else {
        "Nothing in this project opens a database yet, so this is the dialect your\n\
         conditions are compiled for and nothing else. Brocanth supports Postgres\n\
         only, so `postgres` is the one legal value."
    };
    vec![Setting::text(
        "dialect",
        dialect,
        "The SQL a condition compiles to: postgres, the only dialect.",
    )
    .note(agrees)]
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
//! no other route. Delete the registration in `src/plugins/mod.rs` and
//! delete this file, and the application has no rule algebra and links
//! nothing to have one with.
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
//! let rules = ctx.provided::<Rules, _>().expect("a dispatched request");
//! let condition = PostPolicy.list(&visitor).condition::<Post>(rules.dialect)?;
//! // "SELECT .. FROM posts WHERE " + condition.sql(), bound to condition.binds()
//! ```
//!
//! A write is the same predicate carried into the statement, which is what
//! closes the window between the check and the change. The field mask comes
//! first, before any row is read:
//!
//! ```ignore
//! PostPolicy.writable(&visitor).check(["body"])?;   // which columns
//! let admission = PostPolicy.update(&visitor).admission::<Post>(rules.dialect, id)?;
//! let rows = /* run admission.query(), count the rows */;
//! let verified = admission.resolve(rows)?;          // zero rows is a denial
//! let write = verified.update(rules.dialect, [(PostColumn::Body, "new text".into())])?;
//! // run write.query(); then:
//! write.applied(affected)?;                          // zero affected is RaceLost
//! ```
//!
//! `Verified` is taken by value everywhere it is used, and a debug build fails
//! the test that checked one and then forgot to spend it.
//!
//! # Two masks: create and update
//!
//! `update` decides on the row as stored, not on the row the write leaves.
//! So `author_id = me` is true before the write, whatever the write sets, and
//! only the mask stops an author from giving a post to somebody else. That is
//! why `writable` (an update) leaves out `AuthorId`, and `writable_on_create`
//! puts it in: `create` decides on the proposed row, so a create can name
//! only its author's own id. A create through `brocanth-resource` uses
//! `writable_on_create`, an update uses `writable`.
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
    /// The text.
    Body,
}}

impl Column for PostColumn {{
    /// Required rather than derived: a changed-field name that is not one of
    /// these is rejected instead of quietly permitted.
    const ALL: &'static [Self] = &[Self::Id, Self::AuthorId, Self::Status, Self::Body];

    fn name(&self) -> &'static str {{
        match self {{
            Self::Id => "id",
            Self::AuthorId => "author_id",
            Self::Status => "status",
            Self::Body => "body",
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

    /// Which columns an update may change. `update` says which rows; this
    /// says which columns.
    fn writable(&self, who: &Visitor) -> FieldMask<PostColumn> {{
        if who.admin {{
            FieldMask::except([PostColumn::Id])
        }} else {{
            // The author edits the body. Never the status, and never the
            // owner: `update` reads the row as stored, so `AuthorId` here
            // would let an author give a post to somebody else.
            FieldMask::only([PostColumn::Body])
        }}
    }}

    /// Which columns a create may set.
    fn writable_on_create(&self, who: &Visitor) -> FieldMask<PostColumn> {{
        if who.admin {{
            return self.writable(who);
        }}
        // A create names its owner, and `create` above admits only the
        // author's own id.
        FieldMask::only([PostColumn::Body, PostColumn::AuthorId])
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
    /// `postgres`, the only dialect. A string here and an enum below: a config file
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
            other => {{
                return Err(PluginError::new(format!(
                    "[policy] dialect is {{other:?}}, which is not a SQL dialect\n\
                     help: set `dialect` under [policy] to postgres"
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
            .map_err(|e| {{
                PluginError::new(format!(
                    "[policy] posts: {{e}}\n\
                     help: fix the rule in `PostPolicy` above: a column is a bare identifier, and every value binds"
                ))
            }})?;
        let sql = condition.sql();
        crate::plugins::say!(info, "[policy] posts, anonymous: {{sql}}"; sql);

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

#[cfg(test)]
mod tests {{
    use super::*;

    fn author(id: i64) -> Visitor {{
        Visitor {{
            account_id: Some(id),
            admin: false,
        }}
    }}

    #[test]
    fn a_create_names_its_author_and_no_other() {{
        let ada = author(7);
        assert_eq!(
            PostPolicy
                .writable_on_create(&ada)
                .check(["author_id", "body"]),
            Ok(())
        );
        // `create` decides on the proposed row.
        let theirs = brocanth_policy::MapRow::new().with(PostColumn::AuthorId, 8_i64);
        let admitted = PostPolicy.create(&ada).admits(&theirs);
        assert!(!admitted.expect("decided in memory"));
    }}

    #[test]
    fn an_update_changes_the_body_and_never_the_author() {{
        let ada = author(7);
        // The mask first, before any row is read: the update rule reads the
        // row as stored, so it cannot refuse a change of owner.
        assert!(PostPolicy.writable(&ada).check(["author_id"]).is_err());
        assert_eq!(PostPolicy.writable(&ada).check(["body"]), Ok(()));

        // Then the witness, spent by the write that carries its condition.
        let admission = PostPolicy
            .update(&ada)
            .admission::<Post>(Dialect::Postgres, 1_i64)
            .expect("the rule compiles");
        let verified = admission.resolve(1).expect("the admission found the row");
        let write = verified
            .update(Dialect::Postgres, [(PostColumn::Body, "new text".into())])
            .expect("the update compiles");
        assert!(write.query().sql().contains("\"author_id\""));
        assert!(write.applied(0).is_err(), "zero rows is a lost race");
    }}
}}
"##
    )
}
