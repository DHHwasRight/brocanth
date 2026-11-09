//! `filters` -- what `brocanth create` writes when somebody asks for typed
//! list filters.
//!
//! The crate is a library: no plugin, no route, no setting. What a project
//! gets is `ctx.filters(&declared)` on the context, which reads the query
//! string through a declaration and answers a refused value with a 400, and
//! the rule an agent must follow: AND the filters with the policy's rule,
//! in the `WHERE`.
//!
//! It needs `policy`, because a filter is a `Pred` over a policy entity's
//! columns, and the rule it is ANDed with comes from there.
//!
//! `outside_runtime`: it writes code that pages call, and nothing that only
//! a running server uses.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{Category, Dep, Manifest, Module, Project, Scaffold, Slot};

/// Typed list filters, as a choice on the `brocanth create` menu.
pub struct Filters;

impl Module for Filters {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "filters",
            "typed filters and a sort from the query string, ANDed with the policy's rule in the WHERE",
        )
        .category(Category::Renderer)
        .outside_runtime()
        .needs(
            "policy",
            "a filter is a `Pred` over a policy entity's columns, ANDed with its list rule",
        )
        .adapts_to(
            "pagination",
            "the rules name `ctx.links()`, whose page links keep the filters",
        )
        .asked_in("data", "Filters and a sort read from the query string")
        .krate(Dep::brocanth("brocanth-filters").note("filters compiled into the policy's WHERE"))
        .absent(
            "brocanth-filters",
            "no typed filters; a list that filters parses its own query string",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        s.at(Slot::PagesItems).block(CTX_FILTERS.to_string());
        s.at(Slot::AgentsSections).block(agents(project));
    }
}

const CTX_FILTERS: &str = r#"/// The filters a list was asked for.
///
/// `allow(dead_code)`: it is here for the first list, and a new project has
/// none.
#[allow(dead_code)]
impl Ctx {
    /// Reads the query string through `declared`. A lookup the list does not
    /// allow, a value that is not its kind, or an unknown sort is a 400 with
    /// a `help:` line.
    ///
    /// ```ignore
    /// let filters = Filters::new()
    ///     .choice("status", PostColumn::Status, &["draft", "published"])
    ///     .int("author", PostColumn::AuthorId)
    ///     .sort("created_at", PostColumn::CreatedAt);
    /// let applied = ctx.filters(&filters)?;
    /// let condition = applied.condition::<Post>(policy.list(&who), Dialect::Postgres)?;
    /// ```
    pub fn filters<C: brocanth_policy::Column>(
        &self,
        declared: &brocanth_filters::Filters<C>,
    ) -> Result<brocanth_filters::Applied<C>, Error> {
        declared
            .parse(&self.query)
            .map_err(|e| Error::new(400, e.to_string()))
    }
}
"#;

fn agents(project: &Project) -> String {
    let links = if project.has("pagination") {
        "\n- Write page links with `ctx.links()`: each keeps the filters and\n\
         \x20 the sort.\n"
    } else {
        ""
    };
    format!(
        "\n## Filters\n\
         \n\
         A list declares its filters once with `brocanth_filters::Filters`,\n\
         over the entity's column enum, and reads them with\n\
         `ctx.filters(&declared)?`.\n\
         \n\
         - Compile with `applied.condition::<E>(policy.list(&who), dialect)`.\n\
         \x20 It ANDs the filters with the rule, so a filter narrows what the\n\
         \x20 rule admits and never widens it.\n\
         - Give the count and the page the same condition. Never filter rows\n\
         \x20 after a paginated load: the total is then wrong and pages overlap.\n\
         - Order with `applied.order_by::<E>(key)`: the primary key comes last,\n\
         \x20 so two pages never share a row.\n\
         - Do not parse a filter from the query string by hand. A name that is\n\
         \x20 not declared names no column, and that is the guarantee.\n{links}"
    )
}
