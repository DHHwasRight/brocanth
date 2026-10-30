//! Nested Cards -- a brocanth application.
//!
//! No command line was asked for, so this binary does one thing. Everything it
//! contains is in `src/app.rs`; `brocanth create --with commands` turns this
//! into a command line with `serve` as one of its commands.

mod app;
mod assets;
mod data;
mod pages;

use std::process::ExitCode;

fn main() -> ExitCode {
    match app::serve("127.0.0.1:3000") {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}
