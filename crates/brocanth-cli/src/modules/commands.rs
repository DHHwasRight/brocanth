//! `commands` -- this project's own command line.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// A `manage` binary whose subcommands are the project's own structs.
///
/// Also what makes a directory a project: it writes `brocanth.toml`, the
/// marker the global binary walks up to find.
pub struct Commands;

impl Module for Commands {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "commands",
            "this project's own command line, `serve` plus whatever you add",
        )
        .asked_in(
            "foundations",
            "Your own command line: `serve`, and whatever you add",
        )
        .krate(Dep::brocanth("brocanth-command"))
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        // `src/commands/mod.rs` and `serve` are still written by
        // `create.rs`. Moving here next.
        s.file("src/commands/routes.rs", routes_rs(project));
        s.at(Slot::CommandsMod).line("pub mod routes;");
    }
}

/// `src/commands/routes.rs`.
///
/// Rails has `bin/rails routes` because a reader's first question about an
/// application is which URLs it answers, and the second is which file to
/// open. Here both are facts the binary already holds: `pages!` read the
/// directory while the crate compiled, so the listing is not a second
/// reading that can disagree with the first.
///
/// Two shapes. With `input` the project also knows what each route takes
/// and gives, because `described()` read the handler signatures, and two
/// more columns say so. Without it there are three columns and no mention
/// of a table this project does not have.
fn routes_rs(project: &Project) -> String {
    let (doc, table) = if project.has("input") {
        (DOC_WITH_SHAPES, TABLE_WITH_SHAPES)
    } else {
        (DOC, TABLE)
    };
    ROUTES_RS.replace("@DOC@", doc).replace("@TABLE@", table)
}

/// The sample listing and what the columns are, for a project with `input`.
const DOC_WITH_SHAPES: &str = r##"//! ```text
//! $ brocanth routes
//! VERB    PATTERN        HANDLER                      TAKES   GIVES
//! GET     /              pages::index::handle         -       -
//! GET     /about         pages::about::handle         -       -
//! POST    /signup        pages::signup::handle_post   Signup  Created
//! ```
//!
//! `TAKES` and `GIVES` are the body a handler reads and the `Json<T>` it
//! returns, from `crate::pages::described()`. A page that writes
//! `-> Result<impl IntoResponse, Error>` says neither, and prints `-`: the
//! wrapper is the opt-in, and these columns report it rather than ask for
//! it."##;

/// The same, for a project that reads no handler signature.
const DOC: &str = r##"//! ```text
//! $ brocanth routes
//! VERB    PATTERN        HANDLER
//! GET     /              pages::index::handle
//! GET     /about         pages::about::handle
//! POST    /signup        pages::signup::handle_post
//! ```"##;

/// `table`, where `described()` exists.
const TABLE_WITH_SHAPES: &str = r##"/// The header, and one row per route.
///
/// Five columns, because this project has `input`: `described()` read every
/// handler's signature while the crate compiled, so what a route takes and
/// gives is known here and worth a column.
fn table() -> (Vec<&'static str>, Vec<Vec<String>>) {
    let described = crate::pages::described();
    let rows = crate::pages::handlers()
        .iter()
        .map(|(verb, pattern, handler)| {
            let shape = described
                .iter()
                .find(|row| row.verb == *verb && row.path == *pattern);
            let takes = shape
                .and_then(|row| row.takes.as_ref())
                .and_then(|node| node.name())
                .unwrap_or("-");
            let gives = shape
                .and_then(|row| row.gives.as_ref())
                .and_then(|node| node.name())
                .unwrap_or("-");
            vec![
                verb.as_str().to_string(),
                (*pattern).to_string(),
                format!("pages::{handler}"),
                takes.to_string(),
                gives.to_string(),
            ]
        })
        .collect();
    (vec!["VERB", "PATTERN", "HANDLER", "TAKES", "GIVES"], rows)
}"##;

/// `table`, where it does not.
const TABLE: &str = r##"/// The header, and one row per route.
///
/// Three columns: a route is a verb, a pattern, and the function that
/// answers it. What each one takes and gives would be a fourth and a fifth,
/// and they arrive with the `input` module -- without it nothing in this
/// project has read a handler's signature.
fn table() -> (Vec<&'static str>, Vec<Vec<String>>) {
    let rows = crate::pages::handlers()
        .iter()
        .map(|(verb, pattern, handler)| {
            vec![
                verb.as_str().to_string(),
                (*pattern).to_string(),
                format!("pages::{handler}"),
            ]
        })
        .collect();
    (vec!["VERB", "PATTERN", "HANDLER"], rows)
}"##;

/// Everything around the two halves that differ.
const ROUTES_RS: &str = r##"//! Every route this application answers.
//!
//! `brocanth routes` builds this project and runs this command, so the
//! listing is the binary's own answer rather than a second reading of the
//! directory. The rows are `crate::pages::handlers()`, read off `src/pages`
//! while the crate compiled: a route is a file, so there is nothing to
//! regenerate and nothing that can fall behind.
//!
@DOC@
//!
//! An argument keeps the rows that contain it, in any column, so
//! `routes signup` is the one page and `routes POST` is every route that
//! writes.

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// List the routes this application answers.
#[cmd]
#[derive(clap::Args)]
pub struct Routes {
    /// Keep only the rows containing this text
    pub filter: Option<String>,
}

impl Command for Routes {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        let (header, mut rows) = table();
        if let Some(filter) = &self.filter {
            rows.retain(|row| row.iter().any(|cell| cell.contains(filter)));
        }
        if rows.is_empty() {
            // An application with no page and a filter that matched nothing
            // both print nothing, and only one of the two is the reader's
            // own doing.
            match &self.filter {
                Some(filter) => println!("no route matches `{filter}`"),
                None => println!("this application answers no route"),
            }
            return Ok(());
        }

        let mut widths: Vec<usize> = header.iter().map(|title| title.chars().count()).collect();
        for row in &rows {
            for (column, cell) in row.iter().enumerate() {
                widths[column] = widths[column].max(cell.chars().count());
            }
        }

        print_row(&widths, header.iter().copied());
        for row in &rows {
            print_row(&widths, row.iter().map(String::as_str));
        }
        Ok(())
    }
}

/// One line, every cell padded to its column but the last.
///
/// The last is left alone, because trailing spaces are invisible until
/// something pipes this into a diff.
fn print_row<'a>(widths: &[usize], cells: impl Iterator<Item = &'a str>) {
    let mut line = String::new();
    for (column, cell) in cells.enumerate() {
        if column > 0 {
            line.push_str("  ");
        }
        if column + 1 == widths.len() {
            line.push_str(cell);
        } else {
            line.push_str(&format!("{cell:<width$}", width = widths[column]));
        }
    }
    println!("{line}");
}

@TABLE@
"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn written(on: &[&str]) -> String {
        let known = crate::modules::keys();
        let project = Project::new("demo", "Demo", on, &known);
        routes_rs(&project)
    }

    #[test]
    fn the_listing_names_the_handler_rather_than_only_the_url() {
        // The column a reader scans to find the file to open. Without it
        // this is a list of URLs somebody has to grep around.
        let out = written(&["commands"]);
        assert!(out.contains("format!(\"pages::{handler}\")"), "{out}");
        assert!(out.contains("crate::pages::handlers()"), "{out}");
    }

    #[test]
    fn a_project_without_input_is_not_told_about_a_table_it_has_not_got() {
        let out = written(&["commands"]);
        assert!(!out.contains("described()"), "{out}");
        assert!(!out.contains("TAKES"), "{out}");
        assert!(out.contains(r#"vec!["VERB", "PATTERN", "HANDLER"]"#), "{out}");
    }

    #[test]
    fn a_project_with_input_gets_what_each_route_takes_and_gives() {
        // `described()` read the signatures while the crate compiled, so
        // the two columns cost a lookup rather than a second source.
        let out = written(&["commands", "input"]);
        assert!(out.contains("crate::pages::described()"), "{out}");
        assert!(
            out.contains(r#"vec!["VERB", "PATTERN", "HANDLER", "TAKES", "GIVES"]"#),
            "{out}"
        );
    }

    #[test]
    fn no_placeholder_survives_into_the_written_file() {
        for on in [&["commands"][..], &["commands", "input"][..]] {
            let out = written(on);
            assert!(!out.contains('@'), "a placeholder was left behind: {out}");
        }
    }
}
