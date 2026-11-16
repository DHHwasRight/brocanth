//! Every module has a page in `docs/modules/`, and every page there is a
//! module.
//!
//! The pages describe the manifests, and a manifest changes in Rust where
//! nobody reads the docs. The page set follows the keys; the `Categories`
//! and `Conflicts` rows and the generated `Capabilities` table follow each
//! manifest ([`crate::explain::page::page_block`]). The prose is written by hand.
//!
//! `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check`
//! rewrites the rows and the table from the manifests and leaves the rest
//! of each page alone.

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

/// The `Categories` and `Conflicts` rows a page must carry, read from the
/// manifests.
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

    vec![("Categories", categories), ("Conflicts", conflicts)]
}

/// What is wrong with one page, and the page as a bless writes it.
///
/// `rows` are the rows of the first table that the manifest fixes; `block`
/// is the generated table. The rows the table replaced
/// ([`crate::explain::page::REPLACED_ROWS`]) must be gone from the first table.
fn page_problems(
    key: &str,
    text: &str,
    rows: &[(&'static str, String)],
    block: &str,
) -> (Vec<String>, String) {
    use crate::explain::page::{BEGIN, END, REPLACED_ROWS};
    let page = format!("docs/modules/{key}.md");
    let mut wrong = Vec::new();
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();

    // The first table: from `| | |` to the first line that is not a row.
    let top = lines.iter().position(|l| l == "| | |");
    let mut table_end = None;
    if let Some(start) = top {
        let end = start
            + lines[start..]
                .iter()
                .position(|l| !l.starts_with('|'))
                .unwrap_or(lines.len() - start);
        let before = lines.len();
        let mut i = 0;
        lines.retain(|line| {
            let inside = (start..end).contains(&i);
            i += 1;
            let replaced = inside
                && REPLACED_ROWS
                    .iter()
                    .any(|label| line.starts_with(&format!("| {label} | ")));
            if replaced {
                let label = line.trim_start_matches("| ").split(" | ").next();
                wrong.push(format!(
                    "{page} has the hand-written row `{}`; the generated table holds it now",
                    label.unwrap_or(line)
                ));
            }
            !replaced
        });
        table_end = Some(end - (before - lines.len()));
    } else {
        wrong.push(format!("{page} has no `| | |` table under its title"));
    }

    for (label, want) in rows {
        let prefix = format!("| {label} | ");
        let row = format!("{prefix}{want} |");
        match lines.iter().position(|l| l.starts_with(&prefix)) {
            Some(i) if lines[i] != row => {
                wrong.push(format!("{page}\n  has:  {}\n  want: {row}", lines[i]));
                lines[i] = row;
            }
            Some(_) => {}
            None => wrong.push(format!("{page} has no `{label}` row")),
        }
    }

    let begin = lines.iter().position(|l| l == BEGIN);
    let end = lines.iter().position(|l| l == END);
    let block_lines: Vec<String> = block.lines().map(str::to_string).collect();
    match (begin, end) {
        (Some(b), Some(e)) if b < e => {
            if lines[b..=e] != block_lines[..] {
                let first = lines[b..=e]
                    .iter()
                    .zip(&block_lines)
                    .find(|(has, want)| has != want)
                    .map_or_else(
                        || "  the table has another number of rows".to_string(),
                        |(has, want)| format!("  has:  {has}\n  want: {want}"),
                    );
                wrong.push(format!(
                    "{page}: the generated table does not match the manifest\n{first}"
                ));
                lines.splice(b..=e, block_lines);
            }
        }
        _ => {
            wrong.push(format!("{page} has no generated `Capabilities` table"));
            let at = table_end.unwrap_or(lines.len());
            let mut insert = vec![String::new()];
            insert.extend(block_lines);
            lines.splice(at..at, insert);
        }
    }
    let mut out = lines.join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    (wrong, out)
}

/// The generated table of one module's page.
fn block_of(module: &dyn brocanth_scaffold::Module) -> String {
    crate::explain::page::page_block(
        &module.manifest(),
        &crate::explain::page::settings_alone(module),
    )
}

/// Each page matches its manifest. With `BROCANTH_BLESS=1`, the rows and
/// the generated table are rewritten from the manifest instead, and the
/// rest of the page is left alone.
#[test]
fn each_page_states_the_categories_conflicts_and_capabilities_of_its_manifest() {
    let bless = std::env::var_os("BROCANTH_BLESS").is_some();
    let mut wrong = Vec::new();
    let mut tables = 0;
    for module in super::ALL {
        let m = module.manifest();
        let path = pages_dir().join(format!("{}.md", m.key));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
        let (problems, fixed) = page_problems(m.key, &text, &expected_rows(&m), &block_of(*module));
        if problems.is_empty() {
            tables += 1;
        } else if bless {
            std::fs::write(&path, fixed).expect("write the page");
            tables += 1;
        } else {
            wrong.extend(problems);
        }
    }
    assert!(
        wrong.is_empty(),
        "{}\nhelp: run `BROCANTH_BLESS=1 cargo test -p brocanth-cli --bin brocanth -- docs_check` \
         and read the diff; edit the prose by hand",
        wrong.join("\n")
    );
    assert_eq!(
        tables,
        super::ALL.len(),
        "every page carries a generated table"
    );
    eprintln!(
        "docs_check: {tables} of {} pages carry a generated table",
        super::ALL.len()
    );
}

/// A manifest edit with no bless fails the check: the page's table is the
/// old manifest's.
#[test]
fn a_manifest_change_without_a_bless_fails() {
    let module = super::find("jobs").expect("the jobs module");
    let m = module.manifest();
    let settings = crate::explain::page::settings_alone(module);
    let block = crate::explain::page::page_block(&m, &settings);
    let text = format!("# `jobs`\n\n| | |\n|---|---|\n| Categories | x |\n\n{block}\n");
    let (problems, _) = page_problems("jobs", &text, &[], &block);
    assert!(problems.is_empty(), "{problems:?}");

    let edited = m.uses::<brocanth_scaffold::CacheStore>("keeps the dead letter in the cache");
    let (problems, fixed) = page_problems(
        "jobs",
        &text,
        &[],
        &crate::explain::page::page_block(&edited, &settings),
    );
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains("does not match the manifest"),
        "{}",
        problems[0]
    );
    assert!(
        fixed.contains("`CacheStore` (a cache): keeps the dead letter"),
        "{fixed}"
    );
}

/// A bless takes the replaced rows out of the first table and puts the
/// generated table under it.
#[test]
fn a_bless_moves_the_hand_rows_into_the_generated_table() {
    let text = "# `x`\n\nGist.\n\n| | |\n|---|---|\n| Categories | **Enhancer** |\n\
                | Needs | none |\n| Adapts to | none |\n| Crates | none |\n\n## What it writes\n";
    let block = "<!-- generated: capabilities -->\nT\n<!-- end generated: capabilities -->";
    let (problems, fixed) = page_problems("x", text, &[], block);
    assert_eq!(problems.len(), 3, "{problems:?}");
    assert_eq!(
        fixed,
        "# `x`\n\nGist.\n\n| | |\n|---|---|\n| Categories | **Enhancer** |\n| Crates | none |\n\n\
         <!-- generated: capabilities -->\nT\n<!-- end generated: capabilities -->\n\n## What it writes\n"
    );
}
