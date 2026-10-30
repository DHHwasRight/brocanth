//! The one decision, made once.
//!
//! A command-line tool talks to two audiences that want opposite things. A
//! person at a terminal wants colour, alignment, a spinner that stays on one
//! line, and a prompt when something is missing. A pipe, a log file and CI
//! want bytes that do not move and do not carry escape sequences.
//!
//! Every tool that scatters `if is_terminal` through its commands ends up with
//! one command that forgot. So the question is asked exactly once, in
//! [`Face::decide`], and the answer is carried by a [`Ui`] that every command
//! writes through. Nothing in `commands/` calls `println!`.
//!
//! The mechanism is deliberately dull: **the renderers always emit ANSI**, and
//! [`anstream::AutoStream`] strips it on the way out when the face is plain.
//! One code path produces both transcripts, so the plain one cannot drift from
//! the coloured one, and a test can assert the exact bytes by constructing a
//! [`Ui::capture`].
//!
//! Colour vocabulary, and the whole of it:
//!
//! | Style | Means | Used for |
//! |---|---|---|
//! | [`ACCENT`] | the reader acts on this | paths, routes, the thing just made |
//! | [`DIM`] | secondary | labels, column headers, provenance |
//! | [`OK`] / [`WARN`] / [`BAD`] | state | a result, never a noun |
//!
//! One accent and one dim, plus three states. A second accent would mean the
//! first one stopped meaning anything.

use std::cell::RefCell;
use std::fmt::Display;
use std::io::{IsTerminal, Write};

use anstream::{AutoStream, ColorChoice};
use anstyle::{AnsiColor, Color, Style};
use unicode_width::UnicodeWidthStr;

/// The one accent, as the nearest ANSI slot rather than a truecolour triple --
/// a 256-colour terminal and a light background both have to stay legible, and
/// the user's own palette is better tuned to their screen than ours is.
pub const ACCENT: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Cyan)));
/// Secondary text. Never carries meaning on its own.
pub const DIM: Style = Style::new().dimmed();
/// Emphasis without colour, so it survives a monochrome terminal.
pub const STRONG: Style = Style::new().bold();
pub const OK: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Green)));
pub const WARN: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Yellow)));
pub const BAD: Style = Style::new().fg_color(Some(Color::Ansi(AnsiColor::Red)));
/// No style. Spelled out so a call site never has to think about it.
pub const PLAIN: Style = Style::new();

/// Wraps `text` in `style`'s escape sequences.
///
/// Always, regardless of the face: the stream strips them if it must. Width is
/// never measured on the result -- see [`Cell`].
pub fn paint(style: Style, text: impl Display) -> String {
    let text = text.to_string();
    // Painting nothing produces nothing. Without this an empty cell is two
    // escape sequences, which is invisible but is not whitespace, so the
    // trailing-space trim in `Table::render` would step over it.
    if text.is_empty() {
        return text;
    }
    format!("{style}{text}{}", style.render_reset())
}

/// Who is on the other end, as a ladder: each rung does everything the rung
/// below it does, and one thing more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Both descriptors are a terminal. Everything [`Face::Terminal`] can do,
    /// and one thing more -- the alternate screen and a key at a time -- so a
    /// missing answer can be asked as a full-screen chooser rather than as a
    /// line of prompts.
    Tui,
    /// Stdout is a terminal: colour, and a line that can be rewritten in place
    /// so a spinner stays on one row. Stdin is *not*, so nothing may be asked.
    /// `echo "" | brocanth create` lands here and must fail naming the flag
    /// rather than block on a read that will never return.
    Terminal,
    /// A pipe, a file, CI, `NO_COLOR`, `--no-color` or `--quiet`. Plain text,
    /// no animation, and a missing answer is an error rather than a question.
    Plain,
}

impl Face {
    /// The decision. Everything else in this crate reads the result.
    ///
    /// The order is the order of least surprise: an explicit flag outranks an
    /// environment variable, which outranks what the file descriptor looks
    /// like.
    pub fn decide(no_color: bool, quiet: bool) -> Self {
        // `--quiet` is listed here and not only on the progress code because a
        // person who asked for less does not want a spinner redrawing either.
        if no_color || quiet {
            return Face::Plain;
        }
        // https://no-color.org: set, to any value, means do not colour.
        if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Face::Plain;
        }
        // The matching opt-in, for someone piping into a pager that renders
        // ANSI. Checked before the terminal test, which is the whole point.
        // It forces colour and nothing else: a pager cannot answer a question,
        // so this rung is `Terminal` and never `Tui`.
        if std::env::var_os("CLICOLOR_FORCE").is_some_and(|v| !v.is_empty() && v != "0") {
            return Face::Terminal;
        }
        // Stdout, not stderr: a command's output is the thing being captured,
        // and a split decision would colour half a transcript.
        if !std::io::stdout().is_terminal() {
            return Face::Plain;
        }
        // Stdin decides the top rung, and only stdin: reading a key needs a
        // terminal on the *input* side, and the two descriptors are redirected
        // separately. This is the distinction the two-rung version could not
        // make, which is why `echo "" | brocanth new` used to reach a prompt.
        if std::io::stdin().is_terminal() {
            Face::Tui
        } else {
            Face::Terminal
        }
    }

    /// Whether an escape sequence survives to the other end.
    pub fn can_paint(self) -> bool {
        matches!(self, Face::Tui | Face::Terminal)
    }

    /// Whether a question may be asked at all. False means every answer has to
    /// arrive as a flag, and a missing one is an error naming that flag.
    pub fn can_ask(self) -> bool {
        self == Face::Tui
    }

    fn choice(self) -> ColorChoice {
        if self.can_paint() {
            ColorChoice::Always
        } else {
            ColorChoice::Never
        }
    }
}

type Sink = AutoStream<Box<dyn Write>>;

/// Everything the tool says, and the only thing that says it.
pub struct Ui {
    face: Face,
    quiet: bool,
    debug: bool,
    err: RefCell<Sink>,
}

impl Ui {
    /// The real streams.
    pub fn new(face: Face, quiet: bool, debug: bool) -> Self {
        let choice = face.choice();
        Self {
            face,
            quiet,
            debug,
            err: RefCell::new(AutoStream::new(
                Box::new(std::io::stderr()) as Box<dyn Write>,
                choice,
            )),
        }
    }

    /// Whether a prompt may be shown. False means every answer has to arrive
    /// as a flag, and a missing one is an error naming that flag.
    pub fn interactive(&self) -> bool {
        self.face.can_ask()
    }

    // -- the raw writes -----------------------------------------------------

    /// Detail for somebody writing a script that drives this tool.
    ///
    /// There is one caller and there should stay one: the chooser, reporting
    /// the flags it stood in for. A run that was already driven by flags has
    /// nothing to say back, and a trace of what the tool is doing internally
    /// is a different feature that nobody asked for.
    ///
    /// Stderr and off unless `--debug`, so it never reaches a pipe that was
    /// not asked to receive it. `--quiet` does not suppress it: a person who
    /// asked for both wants the detail without the commentary, and reading
    /// `--quiet` as "also drop what I just asked for" would be the tool
    /// arguing with them.
    pub fn debug(&self, text: impl Display) {
        if !self.debug {
            return;
        }
        let mut err = self.err.borrow_mut();
        let _ = writeln!(err, "{}", paint(DIM, format!("debug: {text}")));
    }

    /// Stderr, so it never lands in a redirect, and silent under `--quiet`.
    pub fn note(&self, text: impl Display) {
        if self.quiet {
            return;
        }
        let mut err = self.err.borrow_mut();
        let _ = writeln!(err, "{text}");
        let _ = err.flush();
    }

    /// The product of a command that prints one, such as `dbshell --print`.
    /// Stdout and plain, so a pipe or `$(...)` gets it, and never suppressed:
    /// `--quiet` drops the commentary, not the answer.
    pub fn result(&self, text: impl Display) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{text}");
        let _ = out.flush();
    }

    /// A blank line of commentary. Collapses under `--quiet` with the rest.
    pub fn gap(&self) {
        self.note("");
    }

    // -- the vocabulary -----------------------------------------------------

    /// The one-line result of a command. `✔` plus a sentence, the operative
    /// noun in the accent.
    pub fn done(&self, text: impl Display) {
        self.note(format!("{} {text}", paint(OK, "\u{2714}")));
    }

    /// Something to act on that is not fatal.
    pub fn warn(&self, text: impl Display) {
        self.note(format!("{} {text}", paint(WARN, "!")));
    }

    /// A command's heading: its name, and what it is operating on.
    pub fn heading(&self, name: &str, subject: impl Display) {
        self.note(format!(
            "{} {}",
            paint(STRONG, format!("brocanth {name}")),
            paint(DIM, subject)
        ));
        self.gap();
    }

    /// Aligned columns, to stderr -- for a summary block, which is commentary
    /// on the work rather than the work's output.
    pub fn table_note(&self, table: &Table) {
        for line in table.render() {
            self.note(line);
        }
    }

    /// A diagnostic in rustc's shape. Stderr, and never suppressed: `--quiet`
    /// means "only the findings", not "no findings".
    pub fn diagnostic(&self, d: &Diagnostic) {
        let mut err = self.err.borrow_mut();
        // A blank line after each, as rustc does: two diagnostics run together
        // read as one with a confusing second half.
        let _ = write!(err, "{}", d.render());
        let _ = writeln!(err);
        let _ = err.flush();
    }
}

// -- columns ----------------------------------------------------------------

/// Display width, which is not byte length and not `char` count.
///
/// `{:<20}` pads by `char` count, so a CJK handler name -- two columns per
/// character -- pushes every following column right by its own length.
/// Measuring here is the difference between a grid and a suggestion of one.
pub fn width_of(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Left,
    Right,
}

/// One cell, as styled runs. Width is the sum of the runs' *text* widths; the
/// escape sequences are added after padding, never measured.
#[derive(Debug, Clone)]
pub struct Cell {
    parts: Vec<(String, Style)>,
    align: Align,
}

impl Cell {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            parts: vec![(text.into(), PLAIN)],
            align: Align::Left,
        }
    }

    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            parts: vec![(text.into(), style)],
            align: Align::Left,
        }
    }

    pub fn right(mut self) -> Self {
        self.align = Align::Right;
        self
    }

    pub fn width(&self) -> usize {
        self.parts.iter().map(|(t, _)| width_of(t)).sum()
    }

    fn render(&self, to: usize) -> String {
        let body: String = self
            .parts
            .iter()
            .map(|(t, s)| paint(*s, t))
            .collect::<Vec<_>>()
            .concat();
        let pad = " ".repeat(to.saturating_sub(self.width()));
        match self.align {
            Align::Left => format!("{body}{pad}"),
            Align::Right => format!("{pad}{body}"),
        }
    }
}

/// Rows of cells, rendered into a grid whose columns are as wide as their
/// widest cell and no wider.
#[derive(Debug, Default)]
pub struct Table {
    rows: Vec<Vec<Cell>>,
    indent: usize,
    gap: usize,
}

impl Table {
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            indent: 2,
            gap: 2,
        }
    }

    pub fn indent(mut self, indent: usize) -> Self {
        self.indent = indent;
        self
    }

    pub fn row(&mut self, cells: Vec<Cell>) -> &mut Self {
        self.rows.push(cells);
        self
    }

    pub fn render(&self) -> Vec<String> {
        let columns = self.rows.iter().map(Vec::len).max().unwrap_or(0);
        let mut widths = vec![0usize; columns];
        for row in &self.rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.width());
            }
        }
        let pad = " ".repeat(self.indent);
        let gap = " ".repeat(self.gap);
        self.rows
            .iter()
            .map(|row| {
                let last = row.len().saturating_sub(1);
                let cells: Vec<String> = row
                    .iter()
                    .enumerate()
                    .map(|(i, c)| {
                        // A left-aligned final column needs no padding, and
                        // trailing spaces are invisible on a terminal and
                        // noise in a diff. A right-aligned one still does:
                        // its padding is on the left, where it is the
                        // alignment.
                        let to = if i == last && c.align == Align::Left {
                            0
                        } else {
                            widths[i]
                        };
                        c.render(to)
                    })
                    .collect();
                format!("{pad}{}", cells.join(&gap)).trim_end().to_string()
            })
            .collect()
    }
}

// -- diagnostics ------------------------------------------------------------

/// rustc's shape, because it is the shape every Rust developer already reads
/// and because `error[code]` makes a finding greppable.
///
/// Almost every finding here is about a *route* or a *plugin* rather than a
/// byte range, and the subset that has a real span -- a `mount` call, an
/// `offer` call -- needs the header, the arrow and a caret run. That is the
/// fifty lines below rather than a dependency and a conversion layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
    /// The colour a finding of this severity is counted in.
    pub fn style(self) -> Style {
        match self {
            Severity::Error => BAD.bold(),
            Severity::Warning => WARN.bold(),
        }
    }
}

/// The source line a diagnostic points at.
#[derive(Debug, Clone)]
pub struct Snippet {
    pub line: usize,
    pub column: usize,
    pub text: String,
    /// How many columns the caret run covers. Zero means one caret.
    pub span: usize,
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    /// The rule that failed, e.g. `middleware/unknown`. Becomes
    /// `error[middleware/unknown]`.
    pub code: Option<String>,
    pub message: String,
    /// A file path, a `path:line:col`, or a route. Whatever names the thing.
    pub location: Option<String>,
    pub snippet: Option<Snippet>,
    /// `("help", ...)` or `("note", ...)`, rendered as rustc's `= help:`.
    pub notes: Vec<(&'static str, String)>,
}

impl Diagnostic {
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: Some(code.into()),
            message: message.into(),
            location: None,
            snippet: None,
            notes: Vec::new(),
        }
    }

    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(code, message)
        }
    }

    pub fn at(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    pub fn note(mut self, text: impl Into<String>) -> Self {
        self.notes.push(("note", text.into()));
        self
    }

    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.notes.push(("help", text.into()));
        self
    }

    /// The whole diagnostic, newline-terminated.
    pub fn render(&self) -> String {
        let severity = self.severity;
        let head = match &self.code {
            Some(code) => format!("{}[{code}]", severity.label()),
            None => severity.label().to_string(),
        };
        let mut out = format!(
            "{}{} {}\n",
            paint(severity.style(), head),
            paint(STRONG, ":"),
            paint(STRONG, &self.message)
        );

        // The gutter is as wide as the largest line number, which is rustc's
        // rule and the reason its output stays aligned as a file grows.
        let gutter = self
            .snippet
            .as_ref()
            .map(|s| s.line.to_string().len())
            .unwrap_or(1);
        let pad = " ".repeat(gutter);
        let rule = paint(DIM, "|");

        if let Some(location) = &self.location {
            out.push_str(&format!(
                "{pad}{} {}\n",
                paint(DIM, "-->"),
                paint(PLAIN, location)
            ));
        }
        if let Some(snippet) = &self.snippet {
            let column = snippet.column.max(1);
            let carets = "^".repeat(snippet.span.max(1));
            // The caret offset is a display width too: a tab or a wide
            // character before the span moves the caret.
            let before: String = snippet.text.chars().take(column - 1).collect();
            out.push_str(&format!("{pad} {rule}\n"));
            out.push_str(&format!(
                "{} {rule} {}\n",
                paint(DIM, snippet.line),
                snippet.text
            ));
            out.push_str(&format!(
                "{pad} {rule} {}{}\n",
                " ".repeat(width_of(&before)),
                paint(severity.style(), carets)
            ));
        }
        for (kind, text) in &self.notes {
            out.push_str(&format!(
                "{pad} {} {} {text}\n",
                paint(DIM, "="),
                paint(STRONG, format!("{kind}:"))
            ));
        }
        out
    }
}

// -- formatting -------------------------------------------------------------

/// `my-blog` -> `My Blog`.
///
/// Here rather than beside either caller because both need it and they must
/// agree: the chooser mirrors it into the name field as you type, and the flag
/// route uses it as the default title. Two copies would drift.
pub fn title_case(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_measured_by_display_width_not_char_count() {
        // Two CJK characters are two chars and four columns. `{:<4}` would pad
        // them by two more and push the next column right.
        let mut table = Table::new().indent(0);
        table.row(vec![Cell::new("\u{65e5}\u{672c}"), Cell::new("x")]);
        table.row(vec![Cell::new("abcd"), Cell::new("y")]);
        assert_eq!(table.render(), ["\u{65e5}\u{672c}  x", "abcd  y"]);
    }

    #[test]
    fn a_right_aligned_column_puts_the_units_under_each_other() {
        let mut table = Table::new().indent(2);
        table.row(vec![Cell::new("routes"), Cell::new("12").right()]);
        table.row(vec![Cell::new("handlers"), Cell::new("7").right()]);
        assert_eq!(table.render(), ["  routes    12", "  handlers   7"]);
    }

    #[test]
    fn the_last_column_is_never_padded() {
        let mut table = Table::new().indent(0);
        table.row(vec![Cell::new("a"), Cell::new("long value")]);
        table.row(vec![Cell::new("b"), Cell::new("x")]);
        assert_eq!(table.render(), ["a  long value", "b  x"]);
    }

    #[test]
    fn a_diagnostic_without_a_span_names_the_route() {
        let d = Diagnostic::error("route/no-handler", "`/about` answers no verb")
            .at("/about")
            .note("src/pages/about.rs");
        assert_eq!(
            anstream::adapter::strip_str(&d.render()).to_string(),
            concat!(
                "error[route/no-handler]: `/about` answers no verb\n",
                " --> /about\n",
                "  = note: src/pages/about.rs\n",
            )
        );
    }

    #[test]
    fn no_color_in_the_environment_outranks_a_terminal() {
        // The flag path is pure and testable; the env path is read once here
        // and is the reason `decide` takes no `&self`.
        assert_eq!(Face::decide(true, false), Face::Plain);
        assert_eq!(Face::decide(false, true), Face::Plain);
    }
}
