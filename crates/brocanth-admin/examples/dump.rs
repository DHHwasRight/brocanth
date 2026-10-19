//! Prints the shipped screens, so the HTML can be read rather than guessed at.
//!
//! `cargo run -p brocanth-admin --example dump -- post` prints the list screen;
//! `-- post new` prints the create form.

use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, Principal, Submission};

fn main() {
    let admin = Admin::new(
        Arc::new(Fixture::blog()),
        AdminConfig::new()
            .titled("Blog admin")
            .at("/admin")
            .discovering(Discovery::new().under("blog::models"))
            .with_policy(AllowAllUnsafeForDevelopment)
            .model("post", |m| {
                m.list_columns(["id", "title", "author_id", "published", "views"])
                    .searchable(["title", "body"])
                    .display(|row| {
                        row.get("title")
                            .map(ToString::to_string)
                            .unwrap_or_default()
                    })
            })
            .model("user", |m| {
                m.display(|row| row.get("name").map(ToString::to_string).unwrap_or_default())
            }),
    );

    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = match args.len() {
        0 => "/admin".to_string(),
        _ => format!("/admin/{}", args.join("/")),
    };
    let response =
        block_on(admin.dispatch(Principal::anonymous(), "GET", &path, "", &Submission::new()))
            .expect("the admin rendered");

    println!("{}", response.html().unwrap_or_default());
}
