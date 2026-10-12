//! `brocanth check` -- everything the compiler will not tell you, then the
//! compiler.
//!
//! Four layers, cheapest first, and all of them run so that one command
//! reports every problem rather than the first one.
//!
//! 1. **The route table, through the real matcher.** The templates are fed to
//!    `RouterBuilder`, so a conflict is the engine's verdict and not this
//!    file's opinion, and a page that declares no handler is named rather than
//!    quietly serving a 404.
//! 2. **The mounts against the offers.** §8 says a plugin offers middleware and
//!    only the application mounts it. That split is exactly what makes a typo
//!    in a mount possible, so it is checked: a name nobody offers, and an offer
//!    nobody mounts.
//! 3. **The patterns against the routes.** A middleware mounted at a pattern no
//!    route matches is dead weight that still reads as protection, which is the
//!    worst kind of wrong.
//! 4. **`cargo check`**, last, because the three above cost milliseconds and
//!    this one costs the compile budget.

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use brocanth::routing::{BuildError, Router, ScopePattern, Verb};
use clap::Args;

use crate::commands::{build, Outcome};
use crate::project::{nearest, Project};
use crate::ui::{self, plural, Cell, Diagnostic, Severity, Table, Ui, ACCENT, DIM};

#[derive(Args)]
pub struct CheckArgs {
    /// Do not run cargo; report only what can be read from the sources
    #[arg(long)]
    pub no_compile: bool,
    /// Check the optimised profile too
    #[arg(long)]
    pub release: bool,
    /// Where the pages live, relative to the package root
    #[arg(long, value_name = "DIR")]
    pub pages: Option<String>,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: CheckArgs) -> Outcome {
    let started = Instant::now();
    let project = Project::discover(dir, args.pages.as_deref())?;
    ui.heading("check", project.display(&project.manifest_path));

    let labels: &[&str] = if args.no_compile {
        &["routes", "middleware"]
    } else {
        &["routes", "middleware", "compile"]
    };
    let steps = ui.steps(labels);
    let mut found: Vec<Diagnostic> = Vec::new();

    // 1. The route table.
    steps.begin("routes");
    let (routes, problems) = project.routes();
    found.extend(problems);
    let mut builder = Router::<()>::builder();
    let mut rows = 0usize;
    for route in &routes {
        if route.verbs.is_empty() {
            let mut d = Diagnostic::warning(
                "route/no-handler",
                format!("`{}` answers no verb", route.template.pattern()),
            )
            .at(format!("{}/{}", project.pages_rel, route.file))
            .note(
                "the file names a route, so the path exists; with no handler it can only 404",
            );
            d = match route.hooks.first() {
                // `handle_json` is content negotiation, not a method. A page
                // with only hooks is the mistake this note exists for.
                Some(hook) => d.help(format!(
                    "`{hook}` is a content-negotiation hook, not a verb; add `fn {}`",
                    Verb::Get.handler_fn()
                )),
                None => d.help(format!(
                    "add `pub fn {}(...)`, or delete the file",
                    Verb::Get.handler_fn()
                )),
            };
            found.push(d);
            continue;
        }
        for verb in &route.verbs {
            rows += 1;
            if let Err(e) = builder.route_template(route.template.clone(), *verb, ()) {
                found.push(duplicate(&project, route, &e));
            }
        }
    }
    if let Err(e) = builder.build() {
        found.push(conflict(&e));
    }
    steps.done(
        "routes",
        format!("{} \u{b7} {}", plural(routes.len(), "route"), plural(rows, "handler")),
    );

    // 2 and 3. The mounts, the offers and the patterns.
    steps.begin("middleware");
    let mounts = project.mounts();
    let offers = project.offers();
    let plugins = project.registrations();
    let offered: Vec<&str> = offers.iter().map(|o| o.first.value.as_str()).collect();
    let mut sample_paths: Vec<String> = routes.iter().map(|r| r.sample_path()).collect();
    sample_paths.sort();
    sample_paths.dedup();

    for mount in &mounts {
        let name = mount.name();
        if !offered.contains(&name) {
            let mut d = Diagnostic::error(
                "middleware/unknown",
                format!("`{name}` is mounted, but no registered plugin offers it"),
            )
            .at(mount.location_of_second())
            .snippet(mount.snippet_on_second());
            d = match nearest(name, &offered) {
                Some(close) => d.help(format!("did you mean `{close}`?")),
                None if offered.is_empty() => d.help(
                    "no plugin in this application offers middleware; a plugin publishes one \
                     with `catalog.offer(..)` in `configure_router`",
                ),
                None => d.help(format!("offered here: {}", offered.join(", "))),
            };
            found.push(d.note("the application mounts; the plugin only offers. §8"));
        }

        match ScopePattern::parse(&mount.first.value) {
            Ok(scope) => {
                if !sample_paths.iter().any(|p| scope.matches(p)) {
                    found.push(
                        Diagnostic::warning(
                            "middleware/no-match",
                            format!(
                                "`{}` matches no route, so `{name}` never runs",
                                mount.first.value
                            ),
                        )
                        .at(mount.location())
                        .snippet(mount.snippet())
                        .note("a layer that reads as protection and runs on nothing is worse than no layer")
                        .help("a scope covers its own root: `/admin/{*rest}` includes `/admin`"),
                    );
                }
            }
            Err(e) => found.push(
                Diagnostic::error("middleware/pattern", e.to_string())
                    .at(mount.location())
                    .snippet(mount.snippet()),
            ),
        }
    }

    for offer in &offers {
        if !mounts.iter().any(|m| m.name() == offer.first.value) {
            found.push(
                Diagnostic::warning(
                    "middleware/not-mounted",
                    format!(
                        "`{}` is offered, but the application mounts it nowhere",
                        offer.first.value
                    ),
                )
                .at(offer.location())
                .snippet(offer.snippet())
                .note("plugins never autowire: offering is not mounting")
                .help("mount it in the setup block, at the pattern you want"),
            );
        }
    }
    steps.done(
        "middleware",
        format!(
            "{} \u{b7} {} \u{b7} {}",
            plural(plugins.len(), "plugin"),
            plural(offers.len(), "offer"),
            plural(mounts.len(), "mount")
        ),
    );

    // 4. The compiler.
    let mut compiled = true;
    if !args.no_compile {
        steps.begin("compile");
        let at = Instant::now();
        let run = build::cargo(ui, &project, "check", args.release, false)?;
        compiled = run.ok;
        steps.done(
            "compile",
            format!(
                "cargo check in {}{}",
                ui::elapsed(at.elapsed()),
                if run.ok { "" } else { " \u{2014} failed" }
            ),
        );
    }
    steps.end();
    if !compiled {
        found.extend(build::compile_failed(&project, "check"));
    }

    report(ui, &found, started.elapsed())
}

/// The summary, and the exit code.
///
/// Warnings do not fail the command. A page someone is still writing is a
/// warning, and a tool that refused to pass until every draft was finished
/// would be turned off within a week.
fn report(ui: &Ui, found: &[Diagnostic], took: std::time::Duration) -> Outcome {
    for d in found {
        ui.diagnostic(d);
    }
    let errors = found
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .count();
    let warnings = found.len() - errors;

    ui.gap();
    let mut table = Table::new();
    table.row(vec![
        Cell::styled("errors", DIM),
        Cell::styled(errors.to_string(), Severity::Error.style()).right(),
    ]);
    table.row(vec![
        Cell::styled("warnings", DIM),
        Cell::styled(warnings.to_string(), Severity::Warning.style()).right(),
    ]);
    table.row(vec![
        Cell::styled("took", DIM),
        Cell::styled(ui::elapsed(took), ACCENT).right(),
    ]);
    ui.table_note(&table);
    ui.gap();

    if errors > 0 {
        ui.warn(format!("{} \u{b7} nothing was changed", plural(errors, "error")));
        return Ok(ExitCode::FAILURE);
    }
    if warnings > 0 {
        ui.done(format!("no errors \u{b7} {}", plural(warnings, "warning")));
    } else {
        ui.done("nothing to report");
    }
    Ok(ExitCode::SUCCESS)
}

/// Two files producing one route, which the builder catches before the trie
/// does. `about.rs` and `about/index.rs` are the pair that gets written by
/// accident.
fn duplicate(project: &Project, route: &crate::project::Route, e: &BuildError) -> Diagnostic {
    Diagnostic::error("route/duplicate", e.to_string())
        .at(format!("{}/{}", project.pages_rel, route.file))
        .note("two files produced the same route; which handler wins would depend on scan order")
        .help("`about.rs` and `about/index.rs` are the same route -- keep one")
}

/// The matcher's own verdict, reported in its own words.
fn conflict(e: &BuildError) -> Diagnostic {
    Diagnostic::error("route/conflict", e.to_string())
        .note("the verdict is the matcher's, not this tool's: the templates were handed to it")
        .help("two parameters cannot sit at the same position: `[id].rs` and `[slug].rs` are one route")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warnings_pass_and_errors_do_not() {
        let (ui, _) = Ui::capture(crate::ui::Face::Plain, true);
        let zero = std::time::Duration::ZERO;
        assert_eq!(
            format!("{:?}", report(&ui, &[], zero).unwrap()),
            format!("{:?}", ExitCode::SUCCESS)
        );
        let warning = [Diagnostic::warning("x", "y")];
        assert_eq!(
            format!("{:?}", report(&ui, &warning, zero).unwrap()),
            format!("{:?}", ExitCode::SUCCESS),
            "a draft page must not fail the build"
        );
        let error = [Diagnostic::error("x", "y")];
        assert_eq!(
            format!("{:?}", report(&ui, &error, zero).unwrap()),
            format!("{:?}", ExitCode::FAILURE)
        );
    }
}
