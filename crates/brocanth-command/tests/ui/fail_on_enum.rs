//! A command's fields are its arguments, so an enum cannot be one.
use brocanth_command::cmd;

#[cmd]
enum NotACommand {
    A,
    B,
}

fn main() {}
