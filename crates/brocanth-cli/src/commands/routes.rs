//! `brocanth routes` -- the resolved route table.
//!
//! Modelled on `bin/rails routes`, which is the single most-cited
//! developer-experience example in any of the four incumbent frameworks and is
//! nearly free here: the table already exists, as the shape of `src/pages`.
//! Printing it answers "which file answers this URL?", "does this route take
//! POST?" and "is the session middleware really on this path?" without a
//! compile.
//!
//! The middleware column is the part no incumbent can print. Middleware in
//! brocanth attaches to a *pattern*, not to a router, and the application
//! chooses the pattern -- so "which layers run for this route" is a question
//! with an answer, and the answer is worth a column.

use std::path::Path;
use std::process::ExitCode;

use brocanth::routing::{ScopePattern, Verb};
use clap::Args;

use crate::commands::Outcome;
use crate::project::{Project, Route, Site};
use crate::ui::{plural, Cell, Table, Ui, ACCENT, BAD, DIM, OK, PLAIN, WARN};

#[derive(Args)]
pub struct RoutesArgs {
    /// Keep only rows whose verb, path or handler contains this
    #[arg(value_name = "FILTER")]
    pub filter: Option<String>,
    /// Where the pages live, relative to the package root
    #[arg(long, value_name = "DIR")]
    pub pages: Option<String>,
    /// Leave out the middleware column and the mount table
    #[arg(long)]
    pub no_middleware: bool,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: RoutesArgs) -> Outcome {
    let project = Project::discover(dir, args.pages.as_deref())?;
    let (routes, problems) = project.routes();
    let mounts = if args.no_middleware {
        Vec::new()
    } else {
        project.mounts()
    };
    let offers = project.offers();

    ui.heading(
        "routes",
        format!("{} \u{b7} {}", project.package, project.pages_rel),
    );

    let needle = args.filter.as_ref().map(|f| f.to_lowercase());
    let mut table = Table::new();
    let mut header = vec![
        Cell::styled("VERB", DIM),
        Cell::styled("PATH", DIM),
        Cell::styled("HANDLER", DIM),
    ];
    if !args.no_middleware {
        header.push(Cell::styled("MIDDLEWARE", DIM));
    }
    table.row(header);

    let mut shown = 0usize;
    let mut silent = Vec::new();
    for route in &routes {
        if route.verbs.is_empty() {
            silent.push(route);
            continue;
        }
        let layers = applicable(&mounts, route);
        for verb in &route.verbs {
            let handler = format!("{}#{}", route.file, verb.handler_fn());
            if let Some(needle) = &needle {
                let hay = format!(
                    "{} {} {handler}",
                    verb.as_str(),
                    route.template.pattern()
                )
                .to_lowercase();
                if !hay.contains(needle) {
                    continue;
                }
            }
            shown += 1;
            let mut row = vec![
                Cell::styled(verb.as_str(), verb_style(*verb)),
                path_cell(route.template.pattern()),
                Cell::styled(handler, PLAIN),
            ];
            if !args.no_middleware {
                row.push(match layers.is_empty() {
                    true => Cell::styled("\u{2014}", DIM),
                    false => Cell::styled(layers.join(", "), DIM),
                });
            }
            table.row(row);
        }
    }
    ui.table(&table);

    // The mount table, because the route column says *which* layers run and
    // this one says why: the pattern the application chose, and the plugin
    // that offered the layer in the first place.
    if !args.no_middleware && !mounts.is_empty() {
        ui.say("");
        let mut table = Table::new();
        table.row(vec![
            Cell::styled("MIDDLEWARE", DIM),
            Cell::styled("PATTERN", DIM),
            Cell::styled("MOUNTED AT", DIM),
            Cell::styled("OFFERED BY", DIM),
        ]);
        for mount in &mounts {
            let name = mount.name().to_string();
            let offered = offers
                .iter()
                .find(|o| o.first.value == name)
                .map(|o| o.file.clone());
            table.row(vec![
                Cell::styled(name, PLAIN),
                path_cell(&mount.first.value),
                Cell::styled(mount.location(), DIM),
                match offered {
                    Some(file) => Cell::styled(file, DIM),
                    None => Cell::styled("nothing offers it", BAD),
                },
            ]);
        }
        ui.table(&table);
    }

    ui.gap();
    let mut summary = format!(
        "{} \u{b7} {}",
        plural(shown, "row"),
        plural(routes.len() - silent.len(), "route")
    );
    if !args.no_middleware {
        summary.push_str(&format!(" \u{b7} {}", plural(mounts.len(), "mount")));
    }
    if needle.is_some() {
        summary.push_str(&format!(" \u{b7} filtered from {}", verb_rows(&routes)));
    }
    if silent.is_empty() && problems.is_empty() {
        ui.done(summary);
    } else {
        ui.warn(format!(
            "{summary} \u{b7} {} that answer nothing",
            plural(silent.len() + problems.len(), "file")
        ));
        ui.note(format!(
            "  run {} to see each one",
            crate::ui::paint(ACCENT, "brocanth check")
        ));
    }
    // A report, not a judgement: `check` is the command that fails.
    Ok(ExitCode::SUCCESS)
}

/// The colour of a verb is what it does to the server, not which verb it is.
///
/// Safe methods read; the unsafe ones write; `DELETE` removes. Three states,
/// the same three the rest of the tool uses, rather than seven arbitrary hues.
fn verb_style(verb: Verb) -> anstyle::Style {
    match verb {
        Verb::Get | Verb::Head | Verb::Options => OK,
        Verb::Post | Verb::Put | Verb::Patch => WARN,
        Verb::Delete => BAD,
    }
}

/// A template with its parameters picked out, so `/users/{id}/posts/{slug}`
/// reads as a shape rather than as a string.
pub fn path_cell(pattern: &str) -> Cell {
    let mut cell = Cell::styled("", PLAIN);
    let mut rest = pattern;
    while let Some(open) = rest.find('{') {
        let Some(close) = rest[open..].find('}') else {
            break;
        };
        cell = cell
            .then(&rest[..open], PLAIN)
            .then(&rest[open..open + close + 1], ACCENT);
        rest = &rest[open + close + 1..];
    }
    cell.then(rest, PLAIN)
}

/// The mounted middleware that covers `route`, in mount order.
///
/// A scope decides by path shape, so the question is asked with a concrete
/// path the route would match -- see [`Route::sample_path`].
fn applicable(mounts: &[Site], route: &Route) -> Vec<String> {
    let path = route.sample_path();
    mounts
        .iter()
        .filter(|mount| {
            ScopePattern::parse(&mount.first.value).is_ok_and(|scope| scope.matches(&path))
        })
        .map(|mount| mount.name().to_string())
        .collect()
}

fn verb_rows(routes: &[Route]) -> String {
    plural(routes.iter().map(|r| r.verbs.len()).sum(), "row")
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth::routing::RouteTemplate;

    fn route(pattern: &str, verbs: &[Verb]) -> Route {
        Route {
            template: RouteTemplate::parse(pattern).unwrap(),
            file: "x.rs".into(),
            verbs: verbs.to_vec(),
            hooks: Vec::new(),
        }
    }

    fn mount(pattern: &str, name: &str) -> Site {
        let arg = |value: &str| crate::project::Arg {
            value: value.to_string(),
            line: 1,
            column: 1,
            text: String::new(),
            width: 1,
        };
        Site {
            first: arg(pattern),
            second: Some(arg(name)),
            file: "src/main.rs".into(),
        }
    }

    #[test]
    fn a_catch_all_mount_covers_every_route_including_the_root() {
        let mounts = [mount("/{*rest}", "request_log")];
        assert_eq!(
            applicable(&mounts, &route("/", &[Verb::Get])),
            ["request_log"]
        );
        assert_eq!(
            applicable(&mounts, &route("/users/{id}", &[Verb::Get])),
            ["request_log"]
        );
    }

    #[test]
    fn a_scoped_mount_covers_only_its_subtree() {
        let mounts = [mount("/admin/{*rest}", "session")];
        assert!(applicable(&mounts, &route("/users/{id}", &[Verb::Get])).is_empty());
        assert_eq!(
            applicable(&mounts, &route("/admin/{*rest}", &[Verb::Get])),
            ["session"]
        );
    }

    #[test]
    fn a_parameter_is_matched_by_shape_not_by_what_it_could_hold() {
        // `/{slug}` must not be read as a literal segment called `{slug}`.
        let mounts = [mount("/{slug}", "page_cache")];
        assert_eq!(
            applicable(&mounts, &route("/about", &[Verb::Get])),
            ["page_cache"]
        );
    }

    #[test]
    fn the_parameters_of_a_path_are_picked_out_without_widening_it() {
        let cell = path_cell("/users/{id}/posts/{*rest}");
        assert_eq!(cell.width(), "/users/{id}/posts/{*rest}".len());
        let mut table = Table::new().indent(0);
        table.row(vec![cell, Cell::new("x")]);
        let rendered = anstream::adapter::strip_str(&table.render()[0]).to_string();
        assert_eq!(rendered, "/users/{id}/posts/{*rest}  x");
    }

    #[test]
    fn a_write_verb_and_a_read_verb_do_not_share_a_colour() {
        assert_ne!(verb_style(Verb::Get), verb_style(Verb::Post));
        assert_ne!(verb_style(Verb::Post), verb_style(Verb::Delete));
        assert_eq!(verb_style(Verb::Get), verb_style(Verb::Head));
    }
}
