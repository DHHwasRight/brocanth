//! The README stays true to the binary.
//!
//! A binary crate has no doctests, so its README cannot compile itself. This
//! test reads what `brocanth --help` and `brocanth add --help` list and
//! checks the README against it, in both directions for the commands: a
//! command the README omits and a command the README invents both fail.

use std::process::Command;

const BROCANTH: &str = env!("CARGO_BIN_EXE_brocanth");
const README: &str = include_str!("../README.md");

/// The help of `args`, run outside any project so that no project command
/// is listed.
fn help(args: &[&str]) -> String {
    let outside = std::env::temp_dir();
    let out = Command::new(BROCANTH)
        .arg("-C")
        .arg(&outside)
        .args(args)
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .current_dir(&outside)
        .output()
        .expect("run brocanth");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The names under `Commands:`, without `help`.
fn commands() -> Vec<String> {
    help(&["--help"])
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| line.starts_with("  "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_string)
        .collect()
}

#[test]
fn the_readme_documents_every_command_and_no_other() {
    let listed = commands();
    assert!(listed.contains(&"create".to_string()), "{listed:?}");
    for name in &listed {
        assert!(
            README.contains(&format!("### `brocanth {name}`")),
            "README.md has no `### \\`brocanth {name}\\`` section"
        );
    }
    for line in README.lines() {
        if let Some(rest) = line.strip_prefix("### `brocanth ") {
            let name = rest.trim_end_matches('`');
            assert!(
                listed.iter().any(|l| l == name),
                "README.md documents `brocanth {name}`, which the binary does not have"
            );
        }
    }
}

#[test]
fn the_readme_lists_every_module() {
    let text = help(&["add", "--help"]);
    let modules: Vec<&str> = text
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("- "))
        .filter_map(|rest| rest.split(':').next())
        .collect();
    assert!(modules.len() > 30, "parsed {modules:?} from:\n{text}");
    let table = &README[README.find("## Modules").expect("a Modules section")..];
    for module in modules {
        assert!(
            table.contains(&format!("`{module}`")),
            "README.md's module table does not list `{module}`"
        );
    }
}
