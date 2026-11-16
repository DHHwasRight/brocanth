//! `policy` -- what `brocanth create` writes when somebody asks for rules.
//!
//! The module is here rather than in the CLI because everything it writes is
//! a statement about this crate: that a comparison takes a [`Column`] and
//! never a string, that [`Bound::or_deny`] is a word somebody wrote down,
//! that [`Entity::TABLE`] is an [`Ident`] checked at compile time, and that
//! [`Dialect`] names the one database brocanth supports. Those move when this
//! crate moves, and a generator living in another repository would learn
//! about it from a bug report.
//!
//! The example is written with `policy!`, so the generated dependency turns
//! the `macro` feature on. That is the shape `docs/policy-rls-bridge.md`
//! reads: a hand-written `impl Policy` gets no database policy.
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
//! reason, and it is a small one said plainly: it compiles the example rule
//! once at startup, where a bad column name can still be answered, and it
//! puts the [`Dialect`](crate::Dialect) on the context, so a call site has
//! one place to get it from.
//!
//! It has no setting. `[policy] dialect` used to copy what `[db] url` says,
//! and `docs/module-config.md` Decision 1 says a module never copies another
//! module's choice into a setting. Brocanth supports PostgreSQL only, so the
//! dialect is the constant [`Dialect::Postgres`](crate::Dialect::Postgres).
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
//! Compiled only under the `scaffold` feature, which is off. See the
//! dependency comment in `Cargo.toml`.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

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
            "realtime",
            "the docs say where a rule joins a broadcast key",
        )
        .asked_in(
            "doors",
            "Authorisation rules that compile into the WHERE clause",
        )
        // `macro`: the example is a `policy!` block, the shape the RLS
        // bridge reads.
        .krate(Dep::brocanth("brocanth-policy").feature("macro"))
        .absent(
            "brocanth-policy",
            "no rule algebra; authorisation is whatever each handler remembered to check",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.file("src/plugins/policy/mod.rs", plugin_rs(project));
        s.at(Slot::PluginsMod).line("pub mod policy;");
        // No setting, so no section and no `Settings`: the plugin is built
        // from `()`, in every project. The trait is named in the call, so no
        // file needs it in scope.
        s.at(Slot::AppPlugins).block(
            "        let builder = builder\n\
             \x20           .plugin(<crate::plugins::policy::Policies as brocanth::plugin_api::Plugin>::new(()));\n"
                .to_string(),
        );
        s.at(Slot::PluginsInstalled)
            .line("policy crate::plugins::policy::Policies");
        // A brand-new project reads the dialect, rather than warning that
        // nothing does.
        s.at(Slot::AppContext).block(
            "    let rules: &crate::plugins::policy::Rules = cx.extract();\n\
            \x20   let dialect = &rules.dialect;\n\
            \x20   crate::plugins::say!(info, \"policy: compiling for {dialect:?}\"; ?dialect);"
                .to_string(),
        );
    }
}

fn plugin_rs(project: &Project) -> String {
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
//! one -- what must stay here is the plugin, because that is what puts the
//! dialect on the context.
//!
//! `policy!` writes the column enum (`PostColumn`), the `Entity` impl and the
//! `Policy` impl. `docs/policy.md` has the syntax. Keep the rules in the
//! macro: a hand-written `impl Policy` is one the database cannot read back.
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
//! closes the window between the check and the change. The write takes the
//! field mask too, and refuses a column the mask does not permit:
//!
//! ```ignore
//! let admission = PostPolicy.update(&visitor).admission::<Post>(rules.dialect, id)?;
//! let rows = /* run admission.query(), count the rows */;
//! let verified = admission.resolve(rows)?;          // zero rows is a denial
//! let mask = PostPolicy.writable(&visitor);         // which columns
//! let write = verified.update(rules.dialect, &mask, [(PostColumn::Body, "new text".into())])?;
//! // run write.query(); then:
//! write.applied(affected)?;                          // zero affected is RaceLost
//! ```
//!
//! `Verified` is taken by value everywhere it is used, and a debug build fails
//! the test that checked one and then forgot to spend it. A refused update
//! gives it back: `refused.into_verified()`.
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
use brocanth_policy::{{Dialect, Policy}};

// ---------------------------------------------------------------------------
// A subject, an entity, and the rules about them
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

brocanth_policy::policy! {{
    /// The table the rules below are about. A column is a `PostColumn`, never
    /// a string, and the table name is checked at compile time.
    pub entity Post in "posts" {{
        #[key] id: i64,
        /// Who wrote it.
        author_id: i64,
        /// `draft` or `published`.
        status: String,
        /// The text.
        body: String,
    }}

    /// The rules for `posts`.
    ///
    /// Only `list` is required, because list authorisation is the one that
    /// cannot be done per object: a paginator that filters after reading lies
    /// about its totals. Every rule left out denies, which is what makes
    /// shipping this file as it stands safe rather than lucky.
    pub policy PostPolicy(who: &Visitor) for Post {{
        // `else deny` is the word that says what an anonymous visitor's
        // missing id means: they own nothing.
        list {{
            who.admin => always,
            status == "published" || author_id == who.account_id else deny,
        }}
        create: author_id == who.account_id else deny,
        update {{
            who.admin => always,
            author_id == who.account_id else deny,
        }}
        // Which columns an update may change. `update` says which rows; this
        // says which columns. The author edits the body. Never the status,
        // and never the owner: `update` reads the row as stored, so
        // `author_id` here would let an author give a post to somebody else.
        writable {{
            who.admin => except [id],
            only [body],
        }}
        // Which columns a create may set. A create names its owner, and
        // `create` above admits only the author's own id.
        writable_on_create {{
            who.admin => except [id],
            only [body, author_id],
        }}
    }}
}}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// What this plugin puts on the context.
///
/// The dialect, and only the dialect. Every compile entry point still takes
/// one as an argument -- the call site says which it meant -- and this is
/// where the call site gets it.
///
/// The policies are not here. `PostPolicy` is a unit struct and a value, not
/// a service: a handler writes `PostPolicy.list(&who)` and has nothing to
/// look up.
pub struct Rules {{
    /// Which dialect this application's conditions are compiled for:
    /// Postgres, the only one brocanth supports.
    pub dialect: Dialect,
}}

impl ContextItem for Rules {{}}

/// Compiles the rules once at startup, and puts [`Rules`] on the context.
pub struct Policies {{
    /// Set by `configure`, once the example rule has compiled.
    ready: bool,
}}

impl Plugin for Policies {{
    /// No setting: the dialect is a constant, not a choice.
    type Config = ();
    type Provides = ProvidesContext<Rules>;
    const NAME: &'static str = "policy";

    fn new(_config: ()) -> Self {{
        Self {{ ready: false }}
    }}

    fn configure(&mut self, _lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {{
        // Compile one rule at startup. It costs a string and it answers the
        // only question this file can get wrong without anybody noticing: a
        // column whose name is not a bare identifier fails the query rather
        // than escaping into it, and finding that out here is better than
        // finding it out on the first list request.
        let condition = PostPolicy
            .list(&Visitor::anonymous())
            .condition::<Post>(Dialect::Postgres)
            .map_err(|e| {{
                PluginError::new(format!(
                    "[policy] posts: {{e}}\n\
                     help: fix the rule in `PostPolicy` above: a column is a bare identifier, and every value binds"
                ))
            }})?;
        let sql = condition.sql();
        crate::plugins::say!(info, "[policy] posts, anonymous: {{sql}}"; sql);

        self.ready = true;
        Ok(())
    }}

    fn provide(&self, cx: &mut ContextBuilder) {{
        // `configure` runs before the server is handed the application, so
        // the rules have compiled by the time any request builds a context.
        if self.ready {{
            cx.insert(Rules {{
                dialect: Dialect::Postgres,
            }});
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
        let mask = PostPolicy.writable(&ada);
        let admission = PostPolicy
            .update(&ada)
            .admission::<Post>(Dialect::Postgres, 1_i64)
            .expect("the rule compiles");
        let verified = admission.resolve(1).expect("the admission found the row");

        // The update rule reads the row as stored, so it cannot refuse a
        // change of owner. The mask on the write does, and gives the witness
        // back.
        let refused = verified
            .update(
                Dialect::Postgres,
                &mask,
                [(PostColumn::AuthorId, 8_i64.into())],
            )
            .expect_err("the mask leaves out author_id");
        let verified = refused.into_verified();

        // The witness, spent by the write that carries its condition.
        let write = verified
            .update(
                Dialect::Postgres,
                &mask,
                [(PostColumn::Body, "new text".into())],
            )
            .expect("the update compiles");
        assert!(write.query().sql().contains("\"author_id\""));
        assert!(write.applied(0).is_err(), "zero rows is a lost race");
    }}
}}
"##
    )
}
