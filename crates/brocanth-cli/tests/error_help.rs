//! Every `PluginError` a developer can read carries a `help:` line.
//!
//! The rule is in CLAUDE.md: an error says what to do next. A plugin error
//! stops the start of an application, so it is the error a developer meets
//! first. This test reads two kinds of text:
//!
//! - every golden shape (`crates/*/tests/golden/*.txt`): the code the CLI and
//!   the module crates generate, as a project receives it;
//! - every source file under `crates/*/src`: the runtime plugins, and the
//!   scaffold templates the goldens come from.
//!
//! For each `PluginError::new(…)` and `PluginError::from(…)` it reads the
//! argument. The argument passes when:
//!
//! - it contains `help:`; or
//! - it has no string literal (`e.to_string()`, `message`): the error passes
//!   through a value and adds no sentence of its own; or
//! - its first string literal ends with an interpolation (`"[db] {e}"`,
//!   `"starting the worker: {e}"`): it wraps another error, and that error
//!   owns the help line.
//!
//! The last case cannot be checked here. A wrapped error with no help line
//! is fixed where it is made, in its own crate, so every caller gets it.
//!
//! Test code is skipped: a `#[cfg(test)]` module, and the `tests/` folders.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

/// Every file under `dir` whose name ends with `suffix`, recursively.
fn files(dir: &Path, suffix: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files(&path, suffix, out);
        } else if path.to_string_lossy().ends_with(suffix) {
            out.push(path);
        }
    }
}

/// The text with each `#[cfg(test)]` module replaced by blank lines, so
/// line numbers stay right.
fn without_test_modules(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let next = lines.get(i + 1).map(|l| l.trim_start()).unwrap_or("");
        if line.trim() == "#[cfg(test)]" && next.starts_with("mod ") && next.ends_with('{') {
            // The module ends at the first `}` at its own indentation.
            let indent = &lines[i + 1][..lines[i + 1].len() - next.len()];
            let close = format!("{indent}}}");
            out.push("");
            i += 1;
            while i < lines.len() && lines[i].trim_end() != close {
                out.push("");
                i += 1;
            }
            out.push("");
            i += 1;
            continue;
        }
        out.push(line);
        i += 1;
    }
    out.join("\n")
}

/// The argument of the call that opens at `start`, up to its closing paren.
fn argument(text: &str, start: usize) -> &str {
    let mut depth = 1;
    for (offset, c) in text[start..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return &text[start..start + offset];
                }
            }
            _ => {}
        }
    }
    &text[start..]
}

/// The first string literal in `arg`, as written, without its quotes.
///
/// A template that is itself an ordinary string escapes its quotes as `\"`,
/// so a trailing backslash is dropped.
fn first_literal(arg: &str) -> Option<&str> {
    let open = arg.find('"')?;
    let rest = &arg[open + 1..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if bytes.get(i + 1) == Some(&b'"') => {
                // `\"` at the end of an escaped template closes it.
                let after = rest[i + 2..].trim_start();
                if after.starts_with(')') || after.starts_with(',') || after.is_empty() {
                    return Some(&rest[..i]);
                }
                i += 2;
            }
            b'\\' => i += 2,
            b'"' => return Some(&rest[..i]),
            _ => i += 1,
        }
    }
    Some(rest)
}

/// Why `arg` breaks the rule, if it does.
fn fault(arg: &str) -> Option<&'static str> {
    if arg.contains("help:") {
        return None;
    }
    // No literal: a value passes through, and adds no sentence of its own.
    let literal = first_literal(arg)?;
    if literal.trim_end().ends_with('}') {
        return None;
    }
    Some("no `help:` line")
}

fn faults_in(path: &Path, root: &Path, out: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let text = without_test_modules(&text);
    for call in ["PluginError::new(", "PluginError::from("] {
        let mut from = 0;
        while let Some(found) = text[from..].find(call) {
            let at = from + found;
            let start = at + call.len();
            let arg = argument(&text, start);
            if let Some(why) = fault(arg) {
                let line = text[..at].lines().count().max(1);
                let shown = arg.split_whitespace().collect::<Vec<_>>().join(" ");
                let shown: String = shown.chars().take(160).collect();
                let rel = path.strip_prefix(root).unwrap_or(path).display();
                out.push(format!("{rel}:{line}: {why}: {shown}"));
            }
            from = start;
        }
    }
}

#[test]
fn every_plugin_error_carries_a_help_line() {
    let root = workspace();
    let mut paths = Vec::new();
    let Ok(crates) = fs::read_dir(root.join("crates")) else {
        panic!("no crates/ under {}", root.display());
    };
    let mut crates: Vec<PathBuf> = crates.flatten().map(|e| e.path()).collect();
    crates.sort();
    for krate in &crates {
        files(&krate.join("src"), ".rs", &mut paths);
        files(&krate.join("tests/golden"), ".txt", &mut paths);
    }
    assert!(
        paths
            .iter()
            .any(|p| p.extension().is_some_and(|e| e == "txt")),
        "no golden file found under {}",
        root.display()
    );

    let mut faults = Vec::new();
    for path in &paths {
        faults_in(path, &root, &mut faults);
    }
    faults.sort();
    faults.dedup();
    assert!(
        faults.is_empty(),
        "{} PluginError message(s) without a `help:` line:\n  {}\n\
         help: end each message with `\\nhelp: <what to do next>`: name the setting, \
         the file, the variable or the command. A message that only wraps another \
         error ends with it (`\"[x] {{e}}\"`), and that error carries the line.",
        faults.len(),
        faults.join("\n  ")
    );
}

#[test]
fn the_rule_reads_each_shape_of_message() {
    assert_eq!(fault(r#""[db] url is empty""#), Some("no `help:` line"));
    assert_eq!(fault(r#""[db] url is empty\nhelp: set it""#), None);
    assert_eq!(fault(r#"format!("[db] {e}")"#), None);
    assert_eq!(fault(r#"format!("[db] {{e}}")"#), None);
    assert_eq!(fault(r#"format!(\"[jobs] {e}\")"#), None);
    assert_eq!(
        fault(r#"format!(\"[jobs] gone\")"#),
        Some("no `help:` line")
    );
    assert_eq!(fault("e.to_string()"), None);
    assert_eq!(
        fault(r#"format!("[x] is {other:?}; write a or b")"#),
        Some("no `help:` line")
    );
}

#[test]
fn a_test_module_is_skipped() {
    let text = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { x(\"no\") }\n}\nfn c() {}";
    let kept = without_test_modules(text);
    assert_eq!(kept.lines().count(), text.lines().count());
    assert!(!kept.contains("no"));
    assert!(kept.contains("fn c()"));
}
