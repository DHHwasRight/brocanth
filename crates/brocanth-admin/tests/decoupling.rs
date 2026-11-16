//! The path to decoupling (`docs/module-capabilities.md` §5.3), as tests.
//!
//! The backoffice is a second service, embedded for now. These are the
//! invariants that keep a later split to a new binary and not a rewrite.
//! Each test names the point it holds.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brocanth_admin::late::{Late, LateCells};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{policy_fn, Principal, Request};
use brocanth_admin::rt::block_on;
use brocanth_admin::{Admin, AdminConfig, AdminError, Fixture, Submission};
use brocanth_scaffold::kind::Kind;
use brocanth_scaffold::{AdminPanel, Module, MountRole};

/// The workspace's `crates/` directory.
fn crates() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf()
}

/// Every `.rs` file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The code of a file without its `#[cfg(test)] mod tests` block and its
/// comments, so a test or a doc line that names a global does not count.
fn code(text: &str) -> String {
    let body = text.split("#[cfg(test)]\nmod tests").next().unwrap_or(text);
    body.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Point 1: no tab reads a process-wide global. A tab that does cannot move
/// to another process, and two applications in one process share it.
///
/// The library side of every crate: no `static INSTALLED: OnceLock`, no
/// `installed()`. A tab takes its handle at build or through a [`Late`]
/// cell the admin fills from a request.
#[test]
fn no_crate_keeps_an_installed_global_for_a_tab() {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(crates()).expect("crates/").flatten() {
        sources(&entry.path().join("src"), &mut files);
    }
    assert!(
        files.len() > 100,
        "the walk found the workspace: {}",
        files.len()
    );
    let mut found = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read");
        let code = code(&text);
        // `static INSTALLED: Once` guards a process-wide panic hook or a
        // log subscriber, which are global by nature and serve no tab.
        for needle in [
            "INSTALLED: OnceLock",
            "INSTALLED: std::sync::RwLock",
            "::installed()",
        ] {
            // A generator's text for the file of a project from before the
            // late cells is a string, not code that runs.
            if code.contains(needle) && !code.contains("LEGACY_ANALYTICS_PANEL") {
                found.push(format!("{}: {needle}", file.display()));
            }
        }
    }
    assert!(
        found.is_empty(),
        "tabs read a global:\n{}",
        found.join("\n")
    );
}

/// Point 1, from the other side: the same tab type in two admins of one
/// process reads two handles. A process-wide slot would show one.
#[test]
fn two_admins_in_one_process_read_their_own_handles() {
    struct Count(Late<u32>);

    #[async_trait::async_trait]
    impl Panel for Count {
        fn key(&self) -> &str {
            "count"
        }
        fn label(&self) -> String {
            "Count".to_string()
        }
        async fn render(&self, _r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
            let n = self.0.require("count")?;
            Ok(Some(PanelPage::body("Count", format!("<p>count {n}</p>"))))
        }
    }

    struct Request0(u32);
    let admin = |late: &mut LateCells<Request0>| {
        let cell = late.cell(|r: &Request0| Some(r.0));
        Admin::new(
            Arc::new(Fixture::new()),
            AdminConfig::new()
                .panel(Count(cell))
                .with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
        )
    };
    let (mut one, mut two) = (LateCells::new(), LateCells::new());
    let (a, b) = (admin(&mut one), admin(&mut two));
    one.fill(&Request0(1));
    two.fill(&Request0(2));
    let html = |admin: &Admin| {
        block_on(admin.dispatch(
            Principal::anonymous(),
            "GET",
            "/admin/count",
            "",
            &Submission::new(),
        ))
        .expect("dispatch")
        .html()
        .expect("a page")
        .to_string()
    };
    assert!(html(&a).contains("count 1"));
    assert!(html(&b).contains("count 2"));
}

/// Point 2: the admin is mounted under one `Mount`, with role `Backoffice`,
/// and collects the tabs. Robots, analytics and i18n find it by its role.
#[test]
fn the_admin_is_one_backoffice_mount_that_collects_the_tabs() {
    let m = brocanth_admin::scaffold::Backoffice.manifest();
    let mounts: Vec<_> = m.options.iter().filter(|o| o.mount).collect();
    assert_eq!(mounts.len(), 1, "one mount");
    assert_eq!(mounts[0].role, MountRole::Backoffice);
    assert!(
        m.collects.iter().any(|(k, _)| k.name == AdminPanel::NAME),
        "the admin collects AdminPanel"
    );
}

/// Point 4: the admin ships its own assets. Its stylesheet and script are
/// in the page it renders, so a backoffice on another host needs no file
/// from the application's `public/`.
#[test]
fn the_admin_ships_its_own_assets() {
    let admin = Admin::new(
        Arc::new(Fixture::new()),
        AdminConfig::new().with_policy(policy_fn(|_: &Principal<'_>, _: Request<'_>| true)),
    );
    let page = block_on(admin.dispatch(
        Principal::anonymous(),
        "GET",
        "/admin",
        "",
        &Submission::new(),
    ))
    .expect("dispatch");
    let html = page.html().expect("a page");
    assert!(html.contains("<style"), "the stylesheet is inline");
    assert!(
        !html.contains("href=\"/public/") && !html.contains("src=\"/public/"),
        "nothing from the application's public/: {html}"
    );
}
