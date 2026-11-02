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
