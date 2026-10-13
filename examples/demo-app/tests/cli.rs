//! The CLI this project's own binary presents.
//!
//! These run the real binary, which is the only way to see what the *linker*
//! produced. A unit test can call the runner with a slice built by hand; only
//! a built binary shows whether `#[cmd]` in two separate modules actually
//! reached the registry.

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

fn manage() -> Command {
    Command::cargo_bin("demo-app").expect("the project binary is built")
}

#[test]
fn every_task_in_the_project_is_listed() {
    // Three commands from three places: two modules in this crate and one that
    // came along inside brocanth-command itself. None of them is named in a list
    // anywhere; the linker assembled this.
    manage()
        .arg("commands")
        .assert()
        .success()
        .stdout(contains("seed_demo"))
        .stdout(contains("cache:clear"))
        .stdout(contains("commands"));
}

#[test]
fn the_listing_is_alphabetical_rather_than_whatever_the_linker_chose() {
    let out = manage().args(["commands", "--quiet"]).output().expect("ran");
    let names: Vec<String> = String::from_utf8(out.stdout)
        .expect("utf-8")
        .lines()
        .map(str::to_string)
        .collect();
    // Alphabetical, so the built-in sits between the two of ours
    // rather than last. Renaming it from `tasks` to `commands` moved
    // it, which is exactly what this test exists to notice.
    assert_eq!(names, ["cache:clear", "commands", "seed_demo"]);
}

#[test]
fn a_command_runs_with_its_arguments_parsed() {
    manage()
        .args(["seed_demo", "--count", "5"])
        .assert()
        .success()
        .stdout(contains("5 rows"));
}

#[test]
fn an_argument_has_the_default_its_struct_declared() {
    manage()
        .arg("seed_demo")
        .assert()
        .success()
        .stdout(contains("100 rows"));
}

#[test]
fn help_for_a_task_shows_the_doc_comment_from_its_struct() {
    // `clap::Args::augment_args` copies the struct's doc comment onto the
    // command, so the help text is the one written above the struct rather
    // than a second description kept in step by hand.
    manage()
        .args(["seed_demo", "--help"])
        .assert()
        .success()
        .stdout(contains("Fill the demo database with sample rows"))
        .stdout(contains("--count"))
        .stdout(contains("How many rows to write"));
}

#[test]
fn a_renamed_task_answers_to_the_name_it_was_given() {
    manage()
        .args(["cache:clear", "--force"])
        .assert()
        .success()
        .stdout(contains("cleared"));
}

#[test]
fn a_flag_that_was_not_passed_is_false() {
    manage()
        .arg("cache:clear")
        .assert()
        .success()
        .stdout(contains("pass --force"));
}

#[test]
fn an_unknown_subcommand_fails_rather_than_doing_nothing_quietly() {
    manage().arg("no-such-command").assert().failure();
}

#[test]
fn an_unknown_argument_to_a_real_task_fails() {
    manage().args(["seed_demo", "--nope"]).assert().failure();
}

#[test]
fn no_subcommand_prints_help_rather_than_succeeding_silently() {
    manage()
        .assert()
        .failure()
        .stderr(contains("seed_demo").or(contains("Usage")));
}
