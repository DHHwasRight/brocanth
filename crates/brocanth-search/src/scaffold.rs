//! `search` -- what `brocanth create` writes when somebody asks for site
//! search.
//!
//! What a project gets:
//!
//! - the site index, `search_documents`, as a migration: a SQL file the
//!   `db` module's `migrate` applies, or with `model` a SeaORM migration
//!   that `migrate sync` lists;
//! - `/search`, a page that reads `q` and `page`, ranks the site index
//!   inside a rule, and draws the results with the `pagination` module's
//!   pager: `search.rsx` with `rsx`, a string of HTML without;
//! - the rule an agent must follow: index only what a visitor may read, or
//!   write the rule into the page, where it goes into the `WHERE`.
//!
//! No plugin and no setting: the page reads the pool the `db` plugin put on
//! the context, and the index is a table.
//!
//! Not `outside_runtime`: the page queries Postgres on each request.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::snippet::module_ref;
use brocanth_scaffold::{Category, Dep, Manifest, Module, Opt, Project, Scaffold, Slot};

/// Site search, as a choice on the `brocanth create` menu.
pub struct Search;

impl Module for Search {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "search",
            "Postgres full-text and trigram search: a site index, its migration, a page at /search",
        )
        .category(Category::Routes)
        .needs(
            "db",
            "the index is a table and the page queries it through the `db` pool",
        )
        .needs(
            "pagination",
            "the page reads `page` with `ctx.page_params()` and draws its pager with the module's helpers",
        )
        .adapts_to(
            "model",
            "the migration is a SeaORM file that `migrate sync` lists, not a SQL file",
        )
        .adapts_to(
            "rsx",
            "the page is drawn by src/pages/search.rsx and the `PageNav` component",
        )
        .option(Opt::unsigned(
            "per_page",
            20,
            "Results per page on /search.",
        ))
        .asked_in("data", "Full-text search over a site index, with a page")
        .krate(Dep::brocanth("brocanth-search").note("full-text search in Postgres, and the site index"))
        .absent(
            "brocanth-search",
            "no search; a visitor finds a page by its link or not at all",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        if project.has("model") {
            s.file(MODEL_MIGRATION_PATH, model_migration());
        } else {
            s.file(SQL_MIGRATION_PATH, format!("{SQL_HEADER}{MIGRATION_SQL}"));
        }
        let per_page = module_ref("search", "per_page");
        if project.has("rsx") {
            s.file(
                "src/pages/search.rs",
                SEARCH_RS_RSX.replace("{per_page}", &per_page),
            );
            s.file("src/pages/search.rsx", SEARCH_RSX);
        } else {
            s.file(
                "src/pages/search.rs",
                SEARCH_RS_HTML.replace("{per_page}", &per_page),
            );
        }
        s.at(Slot::PagesRoutes).line("/search");
        s.at(Slot::AgentsSections).block(agents(project));
    }
}

/// The SQL migration, for a project without `model`. A fixed time in its
/// name: the files are applied in name order, and this table depends on
/// nothing, so it may run before or after any other file.
pub const SQL_MIGRATION_PATH: &str = "migrations/20260101_000000_search_documents.sql";

/// The SeaORM migration, for a project with `model`.
pub const MODEL_MIGRATION_PATH: &str = "migrations/m20260101_000000_search_documents.rs";

const SQL_HEADER: &str = "-- The site index of the `search` module: one row per thing a visitor can
-- find. `brocanth_search::documents::upsert` writes a row; /search reads
-- them. The `search` column is generated, so Postgres keeps it in step with
-- `title` and `body` on every write.
--
-- To search another language, change 'english' here and in the page
-- (`documents::index().language(\"french\")`), in a new migration.

";

/// The statements, exactly as `brocanth_search::documents::migration()`
/// writes them. `tests/scaffold.rs` holds the two together.
pub const MIGRATION_SQL: &str = r#"CREATE TABLE "search_documents" (
    "id" bigserial PRIMARY KEY,
    "kind" text NOT NULL,
    "key" text NOT NULL,
    "title" text NOT NULL,
    "body" text NOT NULL DEFAULT '',
    "url" text NOT NULL,
    UNIQUE ("kind", "key")
);
CREATE EXTENSION IF NOT EXISTS pg_trgm;
ALTER TABLE "search_documents" ADD COLUMN "search" tsvector GENERATED ALWAYS AS (
    setweight(to_tsvector('english', coalesce("title", '')), 'A') ||
    setweight(to_tsvector('english', coalesce("body", '')), 'C')
) STORED;
CREATE INDEX "search_documents_search_idx" ON "search_documents" USING gin ("search");
CREATE INDEX "search_documents_title_trgm_idx" ON "search_documents" USING gin ("title" gin_trgm_ops);

-- down
DROP TABLE "search_documents";
"#;

fn model_migration() -> String {
    let (up, down) = MIGRATION_SQL
        .split_once("\n-- down\n")
        .expect("the migration has a down part");
    format!(
        r##"//! The site index of the `search` module: one row per thing a visitor can
//! find. `brocanth_search::documents::upsert` writes a row; /search reads
//! them.
//!
//! Written by `brocanth create`, not by `migrate make`: the generated
//! `tsvector` column and the GIN indexes are not in a model. Run
//! `migrate sync` once, so migrations/mod.rs lists this file.

use brocanth_model::sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {{
    fn name(&self) -> &str {{
        "m20260101_000000_search_documents"
    }}
}}

#[async_trait::async_trait]
impl MigrationTrait for Migration {{
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {{
        manager.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }}

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {{
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }}
}}

const UP: &str = r#"{up}
"#;

const DOWN: &str = r#"{down}"#;
"##
    )
}

const SEARCH_RS_HTML: &str = r#"//! `/search` -- the site search, over `search_documents`.
//!
//! `?q=words&page=2`. The term is read by Postgres' `websearch_to_tsquery`:
//! words, `"a phrase"`, `-not`, `or`. A typo in a title still matches, by
//! trigram.

use brocanth_search::documents::{self, Document, DocumentColumn, Hit};
use brocanth_search::pagination::{query, Pager};
use brocanth_search::policy::{Dialect, Pred};

use crate::pages::{pager_html, Ctx, Error, IntoResponse};

/// Results per page: `per_page` in `src/modules/search.rs`.
const PER_PAGE: u64 = {per_page};

/// GET. An empty `q` shows the form alone.
pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let term = query::param(&ctx.query, "q").unwrap_or_default();
    let params = ctx.page_params()?;
    // The rule. Every row of the site index is public: index only what any
    // visitor may read. To hide rows, write the rule here. It goes into the
    // `WHERE` of the page and of the count, so a hidden row is never ranked.
    let rule = Pred::<DocumentColumn>::Always;
    let condition = rule.condition::<Document>(Dialect::Postgres)?;
    let hits = documents::index()
        .search::<Hit>(ctx.db(), &condition, &term, params.page, PER_PAGE)
        .await?;
    let pager = Pager::new(hits.page, &ctx.links());

    let mut body = format!(
        "<!doctype html><title>Search</title><h1>Search</h1>\
         <form method=\"get\" action=\"/search\" role=\"search\">\
         <input type=\"search\" name=\"q\" value=\"{}\" aria-label=\"Search\"> \
         <button type=\"submit\">Search</button></form>",
        escape(&term)
    );
    if !term.trim().is_empty() {
        if hits.items.is_empty() {
            body.push_str(&format!("<p>Nothing matches “{}”.</p>", escape(&term)));
        } else {
            body.push_str(&format!("<p>{}</p><ol>", pager.summary()));
            for hit in &hits.items {
                body.push_str(&format!(
                    "<li><a href=\"{}\">{}</a><p>{}</p></li>",
                    escape(&hit.url),
                    escape(&hit.title),
                    escape(&hit.excerpt(200))
                ));
            }
            body.push_str("</ol>");
            body.push_str(&pager_html(&pager));
        }
    }
    Ok(body)
}

/// Text and attribute values, escaped: the term came from the request and
/// the rows from whoever wrote the index.
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
"#;

const SEARCH_RS_RSX: &str = r#"//! `/search` -- the site search, over `search_documents`.
//!
//! `?q=words&page=2`. The term is read by Postgres' `websearch_to_tsquery`:
//! words, `"a phrase"`, `-not`, `or`. A typo in a title still matches, by
//! trigram. The markup is in `search.rsx` beside this file.

use brocanth_search::documents::{self, Document, DocumentColumn, Hit};
use brocanth_search::pagination::{query, Pager};
use brocanth_search::policy::{Dialect, Pred};

use crate::pages::{html, views, Ctx, Error, IntoResponse};

/// Results per page: `per_page` in `src/modules/search.rs`.
const PER_PAGE: u64 = {per_page};

/// GET. An empty `q` shows the form alone.
pub async fn handle(ctx: &mut Ctx) -> Result<impl IntoResponse, Error> {
    let term = query::param(&ctx.query, "q").unwrap_or_default();
    let params = ctx.page_params()?;
    // The rule. Every row of the site index is public: index only what any
    // visitor may read. To hide rows, write the rule here. It goes into the
    // `WHERE` of the page and of the count, so a hidden row is never ranked.
    let rule = Pred::<DocumentColumn>::Always;
    let condition = rule.condition::<Document>(Dialect::Postgres)?;
    let hits = documents::index()
        .search::<Hit>(ctx.db(), &condition, &term, params.page, PER_PAGE)
        .await?;
    let pager = Pager::new(hits.page, &ctx.links());
    html!(
        ctx,
        views::search::Search {
            term: &term,
            hits: &hits.items,
            pager: &pager,
        }
    )
}
"#;

const SEARCH_RSX: &str = r#"// The site search. Every value is escaped on the way out: the term came
// from the request and the rows from whoever wrote the index.

use brocanth_search::documents::Hit;
use brocanth_search::pagination::Pager;

use crate::pages::views::parts::pager::PageNav;

pub fn Search(term: &str, hits: &[Hit], pager: &Pager) -> Element {
    <h1>Search</h1>
    <form method="get" action="/search" role="search">
        <input type="search" name="q" value={term} aria-label="Search" />
        <button type="submit">Search</button>
    </form>
    if !term.trim().is_empty() {
        if hits.is_empty() {
            <p>Nothing matches “{term}”.</p>
        } else {
            <p>{pager.summary()}</p>
            <ol>
                for hit in hits {
                    <li>
                        <a href={&hit.url}>{&hit.title}</a>
                        <p>{hit.excerpt(200)}</p>
                    </li>
                }
            </ol>
            <PageNav pager={pager} />
        }
    }
}
"#;

fn agents(project: &Project) -> String {
    let migration = if project.has("model") {
        format!(
            "`{MODEL_MIGRATION_PATH}` creates the table. Run `migrate sync`\n\
             once so `migrations/mod.rs` lists it, then `migrate up`."
        )
    } else {
        format!("`{SQL_MIGRATION_PATH}` creates the table; `migrate up` applies it.")
    };
    format!(
        "\n## Search\n\
         \n\
         `/search` ranks the rows of `search_documents`, the site index.\n\
         {migration}\n\
         \n\
         - Write a row with `brocanth_search::documents::upsert(ctx.db(), kind,\n\
         \x20 key, title, body, url)` when a thing changes, in the same\n\
         \x20 transaction or job, and `documents::remove` when it goes.\n\
         - Every row is public. Index only what any visitor may read, or put\n\
         \x20 the rule in `src/pages/search.rs`: it goes into the `WHERE`, so a\n\
         \x20 hidden row is never ranked, counted or shown.\n\
         - Never filter hits after the page is read. The total and the pages\n\
         \x20 then disagree.\n\
         - To search one table instead, declare a `brocanth_search::Index`\n\
         \x20 over its entity and write `index.migration()` into a migration.\n"
    )
}
