//! `brocanth dev` -- not yet the dev server.
//!
//! The server itself is §4 step 0's other half and belongs to another author.
//! What is settled is the shape, and printing the shape is worth more than
//! printing "not implemented": §6.3 says hot reload does not exist outside
//! Dioxus and `cargo-watch` is archived, so the promise has to be scoped out
//! loud -- templates and assets reload, a handler edit costs a compile. That is
//! the one thing a reader has to know before they start, and this is where they
//! meet it.

use std::path::Path;
use std::process::ExitCode;

use clap::Args;

use crate::commands::Outcome;
use crate::project::Project;
use crate::ui::{Cell, Table, Ui, ACCENT, DIM, STRONG};

#[derive(Args)]
pub struct DevArgs {
    /// Port to listen on
    #[arg(long, default_value_t = 3000, value_name = "PORT")]
    pub port: u16,
    /// Where the pages live, relative to the package root
    #[arg(long, value_name = "DIR")]
    pub pages: Option<String>,
}

pub fn run(ui: &Ui, dir: Option<&Path>, args: DevArgs) -> Outcome {
    let project = Project::discover(dir, args.pages.as_deref())?;
    ui.heading("dev", project.display(&project.manifest_path));

    ui.note(format!(
        "  {}",
        crate::ui::paint(STRONG, "Not built yet. When it is, this command will:")
    ));
    ui.gap();

    let mut table = Table::new().indent(4);
    let mut row = |what: &str, detail: String| {
        table.row(vec![
            Cell::styled(what.to_string(), ACCENT),
            Cell::styled(detail, DIM),
        ]);
    };
    row(
        "serve",
        format!("http://127.0.0.1:{} from the built binary", args.port),
    );
    row(
        "watch",
        format!("{}, templates, assets and config/", project.pages_rel),
    );
    row(
        "reload",
        "templates and assets in place; a handler edit asks cargo".to_string(),
    );
    row(
        "report",
        "the compile duration every time, because it is the budget (\u{a7}6.2)".to_string(),
    );
    row(
        "survive",
        "a failing build leaves the watcher running".to_string(),
    );
    ui.table_note(&table);

    ui.gap();
    ui.note(format!(
        "  {}",
        crate::ui::paint(
            DIM,
            "There is no REPL and there will not be one (\u{a7}6.1). The query console is the \
             mitigation, not a solution."
        )
    ));
    ui.gap();
    ui.warn(format!(
        "`brocanth dev` is a placeholder; run {} for now",
        crate::ui::paint(ACCENT, "brocanth build")
    ));
    // Not a failure: the command did exactly what it says it does.
    Ok(ExitCode::SUCCESS)
}
