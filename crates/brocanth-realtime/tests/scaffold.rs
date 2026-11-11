//! The `realtime` modules, written into a project by the CLI's generator.
//!
//! Each shape is generated, checked against rustfmt, and compared with
//! `tests/golden/<name>.txt`. Bless with `BROCANTH_BLESS=1`; build and test
//! each project too with `BROCANTH_BUILD=1`.

use brocanth_realtime::scaffold::{Realtime, Sse, WebSocket};
use brocanth_scaffold::{testing, Module};

const MODULES: &[&dyn Module] = &[&Realtime, &Sse, &WebSocket];

fn shape(name: &str, picks: &[&'static str]) {
    let files = brocanth_cli::testing::generate("my-blog", picks, MODULES)
        .unwrap_or_else(|e| panic!("{e}"));
    testing::assert_formatted(&files);
    testing::golden(
        format!("{}/tests/golden/{name}.txt", env!("CARGO_MANIFEST_DIR")),
        &testing::snapshot(&files),
    );
    if testing::building() {
        let files = brocanth_cli::testing::generate_for_build("my-blog", picks, MODULES)
            .unwrap_or_else(|e| panic!("{e}"));
        testing::build(&files).unwrap_or_else(|e| panic!("{e}"));
    }
}

/// Both transports in one project, with a config file: two pages under
/// `/realtime`, and one broker.
#[test]
fn both_transports_share_one_broker() {
    shape(
        "realtime-both",
        &["config", "realtime", "realtime-sse", "realtime-websocket"],
    );
}

/// Server-sent events with no config file: the broker's limits come from
/// flags and the environment.
#[test]
fn server_sent_events_without_a_config_file() {
    shape("realtime-sse-flags", &["realtime", "realtime-sse"]);
}
