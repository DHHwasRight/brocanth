//! The commands.
//!
//! Each one returns `Result<ExitCode, Vec<Diagnostic>>`: an exit code when it
//! got far enough to have an opinion, and diagnostics when it did not. Nothing
//! here prints; `main` renders what comes back, through the one [`crate::ui`]
//! layer.

pub mod add;
pub mod check;
pub mod create;
pub mod dbshell;
pub mod delegate;
pub mod migrate;
pub mod project;

/// What every command returns.
pub type Outcome = Result<std::process::ExitCode, Vec<crate::ui::Diagnostic>>;
