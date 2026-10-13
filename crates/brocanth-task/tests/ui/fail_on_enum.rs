//! A task's fields are its arguments, so an enum cannot be one.
use brocanth_task::task;

#[task]
enum NotATask {
    A,
    B,
}

fn main() {}
