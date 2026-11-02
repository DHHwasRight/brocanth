//! Every module has a page in `docs/modules/`, and every page there is a
//! module.
//!
//! The pages describe the manifests, and a manifest changes in Rust where
//! nobody reads the docs. This check does not compare content: it fails
//! when a key is added, renamed or removed and the page set did not follow.
//! The page itself is still updated by hand.

use std::path::PathBuf;

use super::keys;

fn pages_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/modules")
}

/// Every `<key>.md` in `docs/modules/`, without the index.
fn page_keys() -> Vec<String> {
    let dir = pages_dir();
    let entries =
        std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    let mut out: Vec<String> = entries
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter_map(|path| path.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .filter(|stem| stem != "README")
        .collect();
    out.sort();
    out
}

#[test]
fn every_module_key_has_a_page() {
    let pages = page_keys();
    let missing: Vec<&str> = keys()
        .into_iter()
        .filter(|key| !pages.iter().any(|page| page == key))
        .collect();
    assert!(
        missing.is_empty(),
        "no page in docs/modules/ for: {missing:?}\n\
         help: copy docs/modules/cache.md to docs/modules/<key>.md and fill it from the manifest"
    );
}

#[test]
fn every_page_names_a_module_key() {
    let known = keys();
    let orphans: Vec<String> = page_keys()
        .into_iter()
        .filter(|page| !known.contains(&page.as_str()))
        .collect();
    assert!(
        orphans.is_empty(),
        "docs/modules/ has pages for keys no module declares: {orphans:?}\n\
         help: rename the page to the module's new key, or delete it"
    );
}

#[test]
fn the_index_links_every_page() {
    let index = std::fs::read_to_string(pages_dir().join("README.md"))
        .expect("docs/modules/README.md exists");
    let unlinked: Vec<&str> = keys()
        .into_iter()
        .filter(|key| !index.contains(&format!("]({key}.md)")))
        .collect();
    assert!(
        unlinked.is_empty(),
        "docs/modules/README.md does not link: {unlinked:?}\n\
         help: add a row for each key to the table in docs/modules/README.md"
    );
}

/// The `Categories`, `Adapts to` and `Conflicts` rows a page must carry,
/// read from the manifests.
///
/// The first category is the main one and is bold. A conflict comes from a
/// `conflicts` entry on either side, then from the static-site rule, which
/// `brocanth_scaffold::clash` also applies.
fn expected_rows(m: &brocanth_scaffold::Manifest) -> Vec<(&'static str, String)> {
    let link = |key: &str| format!("[`{key}`]({key}.md)");

    let categories = m
        .categories
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let name = format!("{c:?}");
            if i == 0 {
                format!("**{name}**")
            } else {
                name
            }
        })
        .collect::<Vec<_>>()
        .join(", ");

    let adapts = if m.adapts_to.is_empty() {
        "none".to_string()
    } else {
        m.adapts_to
            .iter()
            .map(|(key, _)| link(key))
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut conflicts = Vec::new();
    for other in super::ALL.iter().map(|o| o.manifest()) {
        if other.key == m.key {
            continue;
        }
        let declared = m
            .conflicts
            .iter()
            .find(|(key, _)| *key == other.key)
            .or_else(|| other.conflicts.iter().find(|(key, _)| *key == m.key));
        if let Some((_, why)) = declared {
            conflicts.push(format!("{}: {why}", link(other.key)));
        }
    }
    let needs_server = !m.outside_runtime && !m.static_output;
    if m.static_output {
        conflicts.push(
            "every module that needs a running server (all modules without *outside the runtime*)"
                .to_string(),
        );
    } else if needs_server {
        for site in super::ALL.iter().map(|o| o.manifest()) {
            if site.static_output {
                conflicts.push(format!(
                    "{}: this module needs a running server",
                    link(site.key)
                ));
            }
        }
    }
    let conflicts = if conflicts.is_empty() {
        "none".to_string()
    } else {
        conflicts.join("; ")
    };

    vec![
        ("Categories", categories),
        ("Adapts to", adapts),
        ("Conflicts", conflicts),
    ]
}

/// The page's rows match the manifest. With `BROCANTH_BLESS=1`, the rows
/// are rewritten from the manifest instead, and the rest of the page is
/// left alone.
#[test]
fn each_page_states_the_categories_adaptations_and_conflicts_of_its_manifest() {
    let bless = std::env::var_os("BROCANTH_BLESS").is_some();
    let mut wrong = Vec::new();
    for module in super::ALL {
        let m = module.manifest();
        let path = pages_dir().join(format!("{}.md", m.key));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let mut out = text.clone();
        for (label, want) in expected_rows(&m) {
            let prefix = format!("| {label} | ");
            let Some(line) = text.lines().find(|l| l.starts_with(&prefix)) else {
                wrong.push(format!("docs/modules/{}.md has no `{label}` row", m.key));
                continue;
            };
            let row = format!("{prefix}{want} |");
            if line != row {
                if bless {
                    out = out.replacen(line, &row, 1);
                } else {
                    wrong.push(format!(
                        "docs/modules/{}.md\n  has:  {line}\n  want: {row}",
                        m.key
                    ));
                }
            }
        }
        if bless && out != text {
            std::fs::write(&path, out).expect("write the page");
        }
    }
    assert!(
        wrong.is_empty(),
        "{}\nhelp: fix the page by hand, or run \
         `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check` \
         and read the diff",
        wrong.join("\n")
    );
}
