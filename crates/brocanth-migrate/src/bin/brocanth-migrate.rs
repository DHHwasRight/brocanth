//! The `migrate` tool as a binary of this crate: for its own tests, and for
//! a project that installs it rather than building `src/bin/migrate.rs`.

fn main() -> std::process::ExitCode {
    brocanth_migrate::tool::main()
}
