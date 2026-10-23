//! `client` -- what `brocanth create` writes when somebody asks for a
//! command line that talks to the application.
//!
//! The module is here rather than in the CLI because everything it writes
//! is a statement about this crate: that `run` takes a name, a table, a
//! sender and an argv; that `Reqwest` is the shipped sender and `Wire` is
//! the contract; that a non-2xx answer is an exit code rather than an
//! error. Those move when this crate moves.
//!
//! Compiled only under the `scaffold` feature, which is off.

use brocanth_scaffold::{Dep, Manifest, Module, Project, Scaffold, Slot};

/// The client, as a choice on the `brocanth create` menu.
pub struct Client;

impl Module for Client {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "client",
            "a subcommand that calls this application's own API, built from its routes",
        )
        .asked_in("doors", "A command line that talks to this application")
        .krate(Dep::brocanth("brocanth-client"))
        .needs(
            "input",
            "the client is built from what each page takes and gives, and \
             that table is what `input` writes",
        )
        .needs(
            "commands",
            "the client is a subcommand, and a project with no command line \
             has nowhere to put one",
        )
        .absent(
            "brocanth-client",
            "no client; calling this application means writing the URL and \
             the JSON by hand",
        )
    }

    fn contribute(&self, _project: &Project, s: &mut Scaffold) {
        s.file("src/commands/api.rs", API_RS);
        s.at(Slot::CommandsMod).line("pub mod api;");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// `src/commands/api.rs`.
const API_RS: &str = r##"//! This application, from the outside.
//!
//! `cargo run -- api --help` lists every operation, because the list is
//! `crate::pages::described()` -- the same table the OpenAPI document is
//! built from, read off the handler signatures while this crate compiled.
//! There is nothing to regenerate: add a page and the client grows a
//! subcommand.
//!
//! ```text
//! $ cargo run -- api signup-post --handle ada-lovelace --age 36
//! {"id":"0191a3f0-0000-7000-8000-000000000000","handle":"ada-lovelace"}
//! ```
//!
//! The flags come from the request body's own schema, and their help text
//! is the sentence each field carries. Nothing here re-checks a rule: the
//! server is the authority, and a client that validated too would be a
//! second opinion that can disagree with it.

use brocanth_command::{anyhow, clap, cmd, Command, Context};

/// Call this application's own API.
#[cmd]
#[derive(clap::Args)]
pub struct Api {
    /// The operation and its flags. `api --help` lists them.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub rest: Vec<String>,
}

impl Command for Api {
    fn run(self, _ctx: &mut Context) -> anyhow::Result<()> {
        let routes = crate::pages::described();
        let wire = brocanth_client::Reqwest::new().map_err(anyhow::Error::msg)?;

        let mut argv: Vec<std::ffi::OsString> = vec!["api".into()];
        argv.extend(self.rest.into_iter().map(std::ffi::OsString::from));

        let answered =
            brocanth_client::run("api", &routes, &wire, argv).map_err(anyhow::Error::msg)?;

        println!("{}", answered.body);
        // The status is the exit code, so `api ... && something` means what
        // it looks like in a shell. The body is still printed, because a
        // 422 from this framework carries every fault and that is the part
        // worth reading.
        if !answered.ok() {
            std::process::exit(1);
        }
        Ok(())
    }
}
"##;

/// What the generated `AGENTS.md` says.
const AGENTS: &str = "\n\
    ## The client\n\
    \n\
    `src/commands/api.rs` is a command line for this application's own\n\
    API. It is built from `crate::pages::described()` at run time, not\n\
    generated, so there is nothing to regenerate and nothing to keep in\n\
    step: add a page with a typed signature and the client grows a\n\
    subcommand with the right flags.\n\
    \n\
    Do not add validation to it. The server checks the body and answers\n\
    422 with every fault; a client that checked as well would be a second\n\
    opinion, and the two would disagree the first time a rule changed.\n";
