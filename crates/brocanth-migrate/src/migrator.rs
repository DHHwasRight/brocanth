//! `migrations/mod.rs`: the list of migrations a project's binary applies.
//!
//! The file is stamped the way `brocanth` stamps a file it owns: a header,
//! then the body, then a digest of both. While the digest still matches,
//! the file is the tool's and [`crate::tool`] rewrites it whole. Once a
//! person edits it, the tool leaves it alone and names what it lacks.
//!
//! `brocanth create` writes the first, empty list, and the `migrate` tool
//! writes every later one. Both must write the same bytes for the same
//! list, or the tool would read the CLI's file as edited.
//! `tests/golden/migrator-empty.rs` pins the empty list, and a test on each
//! side compares with it.

/// The list, from the project root.
pub const MIGRATOR: &str = "migrations/mod.rs";

/// The digest of a stamped file, as the file carries it. The same marker
/// `brocanth` writes on every file it owns.
const MARKER: &str = "brocanth-digest:";

/// The two header lines, after `//! `.
const BY: &str = "Written by `brocanth`. Do not edit -- the `migrate` tool rewrites this file.";
const INSTEAD: &str =
    "A migration is a file beside this one; `migrate make` and `migrate sync` list it here.";

/// `migrations/mod.rs`, stamped, for these migration files (module names,
/// oldest first).
pub fn migrator_file(stems: &[String]) -> String {
    stamp(&migrator_rs(stems))
}

/// Whether a list on disk is still the one the tool wrote. `None` for a
/// file that carries no digest, which the caller treats as edited.
pub fn unedited(text: &str) -> Option<bool> {
    let at = text.rfind(MARKER)?;
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let end = text[at..].find('\n').map_or(text.len(), |n| at + n + 1);
    let carried = text[at + MARKER.len()..end]
        .split_whitespace()
        .next()
        .unwrap_or("");
    let head = text[..start].strip_suffix('\n')?;
    let body = format!("{head}{}", &text[end..]);
    Some(digest(&body) == carried)
}

fn stamp(text: &str) -> String {
    let body = format!("//! {BY}\n//! {INSTEAD}\n//!\n{text}");
    let body = if body.ends_with('\n') {
        body
    } else {
        format!("{body}\n")
    };
    format!("{body}\n// {MARKER} {}\n", digest(&body))
}

/// FNV-1a, 64 bits: the digest `brocanth` writes. It answers "has a person
/// edited this file", and nothing more.
fn digest(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The list, before the stamp.
///
/// Written in a shape rustfmt keeps, so a project's own `cargo fmt` leaves
/// the file and its digest alone.
pub fn migrator_rs(stems: &[String]) -> String {
    let mods: String = stems.iter().map(|stem| format!("mod {stem};\n")).collect();
    // One element per line, under a comment. rustfmt keeps a list with a
    // comment in it vertical, so the shape does not depend on how long the
    // names are, and each new migration is a one-line diff.
    let list = if stems.is_empty() {
        "        vec![]\n".to_string()
    } else {
        let mut out = String::from("        vec![\n            // Oldest first.\n");
        for stem in stems {
            out.push_str(&format!("            Box::new({stem}::Migration),\n"));
        }
        out.push_str("        ]\n");
        out
    };
    format!(
        "//! The migrations in this directory, oldest first.\n\
         //!\n\
         //! A file name starts with its UTC time, so the order of the names is the\n\
         //! order the migrations were written. `src/commands/migrate.rs` applies this\n\
         //! list, and `src/main.rs` reaches this file through `#[path]`.\n\
         \n\
         use brocanth_model::sea_orm_migration::prelude::*;\n\
         \n\
         {mods}{gap}\
         /// Every migration this binary applies, oldest first.\n\
         pub struct Migrator;\n\
         \n\
         #[async_trait::async_trait]\n\
         impl MigratorTrait for Migrator {{\n\
        \x20   fn migrations() -> Vec<Box<dyn MigrationTrait>> {{\n\
         {list}\
        \x20   }}\n\
         }}\n",
        gap = if stems.is_empty() { "" } else { "\n" },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_empty_list_is_the_one_brocanth_create_writes() {
        assert_eq!(
            migrator_file(&[]),
            include_str!("../tests/golden/migrator-empty.rs")
        );
    }

    #[test]
    fn a_written_list_reads_as_unedited_and_an_edit_does_not() {
        let text = migrator_file(&names(&["m20261030_120000_create_posts"]));
        assert_eq!(unedited(&text), Some(true));
        assert_eq!(unedited(&format!("{text}// mine\n")), Some(false));
        assert_eq!(unedited(&text.replace("Oldest", "Newest")), Some(false));
        assert_eq!(unedited("mod m1;\n"), None);
    }

    /// The list in every shape it takes, through rustfmt: empty, one short
    /// name, one long name, and several. A project's own `cargo fmt` must
    /// leave the file alone, or the next `make` finds it edited.
    #[test]
    fn the_list_is_already_formatted() {
        use std::io::Write;
        use std::process::{Command, Stdio};

        if Command::new("rustfmt").arg("--version").output().is_err() {
            eprintln!("SKIPPED: rustfmt is not on PATH");
            return;
        }
        for stems in [
            names(&[]),
            names(&["m20261030_120000_a"]),
            names(&["m20261030_120000_create_posts_and_comments"]),
            names(&["m20261030_120000_create_posts_and_comments_and_tags"]),
            names(&[
                "m20261030_120000_create_posts",
                "m20261031_090000_rename_title",
                "m20261101_100000_add_tags",
            ]),
        ] {
            let text = migrator_file(&stems);
            let mut child = Command::new("rustfmt")
                .args(["--edition", "2021", "--emit", "stdout"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("rustfmt");
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout), text, "{stems:?}");
        }
    }
}
