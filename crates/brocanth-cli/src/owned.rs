//! Which generated files the generator may rewrite, and how it proves it may.
//!
//! `brocanth create` writes a project once. `brocanth add` writes into one
//! that already exists, and the two run the same generator, so every file the
//! generator writes has to be one of two things:
//!
//! * **Owned.** The generator rewrites it whole on every `add`. It carries a
//!   header saying so and a digest proving nobody edited it in between.
//! * **The developer's.** Written once, when the project or the module
//!   arrives, and never written again.
//!
//! There is no third kind for Rust source. `Cargo.toml` and
//! `config/<env>.toml` are genuinely shared -- both sides have lines in them
//! -- and `brocanth add` appends to those rather than rewriting them; see
//! [`crate::commands::add`].
//!
//! # Why a digest and not a mtime or a git check
//!
//! The question is "did a person change this file", and the answer has to
//! survive a checkout, a copy, a zip, and a project that is not in git at
//! all. The file's own bytes are the only thing that does.

/// The digest of an owned file, as the file carries it.
const MARKER: &str = "brocanth-digest:";

/// What a file is to the generator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The generator rewrites it. Header, digest, and a refusal when the
    /// digest does not match.
    Owned,
    /// The developer's. Written when it is absent and never again.
    Theirs,
    /// Both sides write into it, so `add` appends rather than rewrites.
    Shared,
}

/// Which of the three a path is.
///
/// A list, deliberately, and a short one. The rule behind it: a file is
/// owned when every byte in it follows from the module list, so a developer
/// who wants to change it is really asking for a different module list. The
/// moment a file has one line of theirs in it, it is theirs.
pub fn kind(path: &str) -> Kind {
    match path {
        // The module list itself, and the per-module record a later
        // `remove` will read.
        "brocanth.toml" => Kind::Owned,
        // `mod assets;` arrives with tailwind, `mod commands;` with the
        // command line. It varies with the module set and with nothing
        // else.
        "src/main.rs" => Kind::Owned,
        // Every `mod.rs` the generator writes under a directory the
        // generator fills. `src/pages/mod.rs` is not one of them: it holds
        // `Response`, `Ctx` and the error contract, which are the
        // developer's to change.
        "src/plugins/mod.rs" | "src/commands/mod.rs" => Kind::Owned,
        // The brief is a list of what this project has and what it
        // therefore does not, and a stale one is worse than none: a model
        // that reads "no queue here" and finds one guesses about
        // everything else too. Every byte of it follows from the module
        // list, so the generator keeps it.
        "AGENTS.md" => Kind::Owned,
        "Cargo.toml" => Kind::Shared,
        path if path.starts_with("config/") && path.ends_with(".toml") => Kind::Shared,
        _ => Kind::Theirs,
    }
}

/// How a file carries a comment.
///
/// Three, because the owned files are Rust, TOML and Markdown, and a `#` at
/// the top of a Markdown file is a heading rather than a note to the
/// reader.
fn comment(path: &str) -> (&'static str, &'static str) {
    if path.ends_with(".toml") {
        ("#", "#")
    } else if path.ends_with(".md") {
        ("<!--", "-->")
    } else {
        ("//!", "//")
    }
}

/// Where to put what the generator would have written, for a file the
/// developer owns. One sentence, on the header of each owned file.
fn instead(path: &str) -> &'static str {
    match path {
        "src/commands/mod.rs" => {
            "A command of your own is a file beside this one; nothing lists it."
        }
        "brocanth.toml" => "Change the module list with `brocanth add`, not here.",
        "AGENTS.md" => "Notes of your own go in a file of your own, beside this one.",
        _ => "Something else belongs here? Put it in `src/app.rs`, which is yours.",
    }
}

/// `text` with the header and the digest on it, ready to write.
pub fn stamp(path: &str, text: &str) -> String {
    let (open, close) = comment(path);
    let (doc, shut, blank) = match close {
        "-->" => (open, " -->", String::new()),
        _ => (open, "", format!("{open}\n")),
    };
    let head = format!(
        "{doc} Written by `brocanth`. Do not edit -- `brocanth add` rewrites this file.{shut}\n\
         {doc} {instead}{shut}\n\
         {blank}",
        instead = instead(path)
    );
    // The header goes above the file's own prose, and the digest goes at the
    // bottom. Not beside the header: a Rust file's `//!` block has to reach
    // the first item without a plain comment cutting it in half, and a rule
    // that holds for one of the two comment syntaxes and not the other is a
    // rule with a bug in it.
    let body = format!("{head}{text}");
    let body = if body.ends_with('\n') {
        body
    } else {
        format!("{body}\n")
    };
    let (open, close) = comment(path);
    let close = if close == "-->" { " -->" } else { "" };
    let open = if open == "//!" { "//" } else { open };
    format!("{body}\n{open} {MARKER} {}{close}\n", digest(&body))
}

/// The body of a stamped file and the digest it carries, or `None` for a
/// file that was never stamped.
///
/// The body is everything except the digest line, so a line appended after
/// it is part of what the digest has to account for. Leaving the tail out
/// would make "append to the bottom of the file" the one edit this check
/// could not see, which is also the most likely one.
pub fn unstamp(text: &str) -> Option<(String, &str)> {
    let at = text.rfind(MARKER)?;
    let start = text[..at].rfind('\n').map_or(0, |n| n + 1);
    let end = text[at..].find('\n').map_or(text.len(), |n| at + n + 1);
    // The first word after the marker. A Markdown file closes its comment
    // after the digest, so taking the rest of the line would take the
    // closer with it.
    let carried = text[at + MARKER.len()..end]
        .split_whitespace()
        .next()
        .unwrap_or("");
    // The blank line `stamp` put between the body and the digest goes too.
    let head = text[..start].strip_suffix('\n')?;
    Some((format!("{head}{}", &text[end..]), carried))
}

/// Whether a file on disk is still the one the generator wrote.
///
/// `None` means the file carries no digest at all, which is a project made
/// by a brocanth older than this one. Treated as an edit, because the
/// alternative is overwriting a file nobody warned.
pub fn unedited(text: &str) -> Option<bool> {
    let (body, carried) = unstamp(text)?;
    Some(digest(&body) == carried)
}

/// FNV-1a, 64 bits, written out here.
///
/// No dependency, and none wanted. `sha2` is already in the lock file and
/// would have done, but this digest answers one question -- "has a person
/// edited this file" -- and it is read by the same tool that wrote it. A
/// cryptographic hash would claim to answer a second question, "did someone
/// forge this", which nothing here asks and which a digest sitting in the
/// file it protects cannot answer anyway.
///
/// `DefaultHasher` would also have done and is explicitly documented not to
/// be stable across Rust releases. This digest is written into other
/// people's projects, so it has to mean the same thing next year.
fn digest(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// A line-by-line diff of two generated texts, for a person to read before
/// they lose one of them.
///
/// Unified-ish, and deliberately not a real unified diff: there is no patch
/// program at the other end of it, only a developer deciding whether the
/// edit they are about to lose mattered.
pub fn diff(before: &str, after: &str) -> String {
    let old: Vec<&str> = before.lines().collect();
    let new: Vec<&str> = after.lines().collect();
    // The common head and tail, so what prints is the part that moved.
    let head = old
        .iter()
        .zip(&new)
        .take_while(|(a, b)| a == b)
        .count()
        .min(old.len().min(new.len()));
    let tail = old[head..]
        .iter()
        .rev()
        .zip(new[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = String::new();
    let context = head.saturating_sub(2);
    for line in &old[context..head] {
        out.push_str(&format!("  {line}\n"));
    }
    for line in &old[head..old.len() - tail] {
        out.push_str(&format!("- {line}\n"));
    }
    for line in &new[head..new.len() - tail] {
        out.push_str(&format!("+ {line}\n"));
    }
    let after_tail = &old[old.len() - tail..];
    for line in after_tail.iter().take(2) {
        out.push_str(&format!("  {line}\n"));
    }
    // A screenful, and then the count. The reader is deciding whether to
    // keep an edit, and a whole file scrolling past is not a decision aid.
    let shown: Vec<&str> = out.lines().collect();
    if shown.len() <= SHOWN {
        return out;
    }
    format!(
        "{}\n  .. and {} more lines\n",
        shown[..SHOWN].join("\n"),
        shown.len() - SHOWN
    )
}

/// How many lines of a diff reach the screen.
const SHOWN: usize = 24;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamped_file_proves_it_was_not_edited() {
        let text = stamp(
            "src/plugins/mod.rs",
            "//! The plugins.\n\npub mod logging;\n",
        );
        assert!(text.starts_with("//! Written by `brocanth`."));
        assert_eq!(unedited(&text), Some(true));

        let edited = text.replace("pub mod logging;", "pub mod logging;\npub mod mine;");
        assert_eq!(unedited(&edited), Some(false));
    }

    #[test]
    fn a_line_appended_below_the_digest_is_still_an_edit() {
        // The likeliest edit of all -- a developer adds something at the
        // bottom -- and the one a digest over the text above it misses.
        let text = stamp(
            "src/plugins/mod.rs",
            "//! The plugins.\n\npub mod logging;\n",
        );
        assert_eq!(unedited(&format!("{text}pub mod mine;\n")), Some(false));
    }

    #[test]
    fn a_file_with_no_digest_is_treated_as_edited_rather_than_as_fresh() {
        // A project made by an older brocanth. Overwriting it because it
        // carries no proof would be the one thing this must never do.
        assert_eq!(unedited("//! mine\n"), None);
    }

    #[test]
    fn a_toml_file_is_stamped_in_its_own_comment_syntax() {
        let text = stamp("brocanth.toml", "[project]\nname = \"blog\"\n");
        assert!(text.starts_with("# Written by `brocanth`."), "{text}");
        assert!(!text.contains("//"), "{text}");
        assert_eq!(unedited(&text), Some(true));
    }

    #[test]
    fn a_markdown_file_is_stamped_in_a_comment_and_not_in_a_heading() {
        // `#` at the top of a Markdown file is a heading, so the brief
        // would open with "Written by brocanth" set in 32 point.
        let text = stamp("AGENTS.md", "# Blog\n\nA brocanth application.\n");
        assert!(text.starts_with("<!-- Written by `brocanth`."), "{text}");
        assert!(text.trim_end().ends_with("-->"), "{text}");
        assert_eq!(unedited(&text), Some(true));
    }

    #[test]
    fn the_digest_is_the_same_number_every_time_this_tool_runs() {
        // It is written into other people's projects, so a change to it is
        // a change every project notices. Pinned rather than described.
        assert_eq!(digest(""), "cbf29ce484222325");
        assert_eq!(digest("brocanth"), "8cfa76d23e49e6c8");
    }

    #[test]
    fn a_diff_shows_the_lines_that_moved_and_not_the_file() {
        let before = "a\nb\nc\nd\ne\nf\ng\n";
        let after = "a\nb\nc\nX\ne\nf\ng\n";
        let shown = diff(before, after);
        assert!(shown.contains("- d\n"), "{shown}");
        assert!(shown.contains("+ X\n"), "{shown}");
        assert!(!shown.contains("- a\n"), "{shown}");
    }

    #[test]
    fn only_the_files_that_follow_from_the_module_list_are_owned() {
        assert_eq!(kind("src/plugins/mod.rs"), Kind::Owned);
        assert_eq!(kind("src/main.rs"), Kind::Owned);
        assert_eq!(kind("brocanth.toml"), Kind::Owned);
        // The response contract lives here, and it is the developer's.
        assert_eq!(kind("src/pages/mod.rs"), Kind::Theirs);
        assert_eq!(kind("src/app.rs"), Kind::Theirs);
        assert_eq!(kind("src/plugins/logging/mod.rs"), Kind::Theirs);
        assert_eq!(kind("Cargo.toml"), Kind::Shared);
        assert_eq!(kind("config/development.toml"), Kind::Shared);
    }
}
