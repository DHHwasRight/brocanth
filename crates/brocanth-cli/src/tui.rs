//! The chooser: one screen, with every answer on it at once.
//!
//! Reached from `brocanth create` when [`crate::ui::Face::Tui`] says both
//! descriptors are a terminal *and* at least one answer is missing. With every
//! answer supplied as a flag this module is never entered, and on anything
//! less than the top rung it is never reachable -- a missing answer there is a
//! diagnostic naming the flag, because a prompt in CI blocks until it times
//! out.
//!
//! # Three decisions worth defending
//!
//! **A form, not a wizard.** The answers interact: ticking `config` changes
//! what the generated `main` reads, and `--core-only` empties the list
//! entirely. A wizard hides each answer behind the next, so the consequence of
//! the first is off-screen by the time you meet the third. One screen costs a
//! little more layout and shows the whole decision.
//!
//! **The chooser can set nothing a flag cannot.** It fills the same [`Draft`]
//! the flags fill, and returns it. There is no answer reachable only by a
//! person with a terminal -- otherwise a scaffold would exist that CI could
//! not reproduce.
//!
//! **Cancelling is an answer.** Escape returns `None`, which the caller must
//! tell apart from "accepted every default". Writing an application because
//! someone pressed Escape is the one unrecoverable mistake available here:
//! the command's next act is to create a directory.
//!
//! # The palette
//!
//! Richer than the rest of the CLI's, and deliberately. Elsewhere output is
//! piped, grepped and pasted into issues, so one accent is right. This screen
//! is only ever reached when both descriptors are a terminal -- `NO_COLOR`
//! and a pipe both send the command down a rung that never gets here -- so
//! it is the one place where colour is certain to arrive.
//!
//! Three hues, and each one means something:
//!
//! | | |
//! |---|---|
//! | [`PINK`] | where you are, and what you are changing |
//! | [`PURPLE`] | the brand, as a background rather than a foreground |
//! | [`FUCHSIA`] | headings, and the rule down the side of a group |
//! | [`GREEN`] | on |
//!
//! The numbers are Lip Gloss's own, not an approximation of them: `#7D56F4`
//! behind `#FAFAFA` is the title block every Bubble Tea tutorial opens with,
//! and `#FF5F87` is what `huh` puts on the row the cursor is on. Copying the
//! palette by eye produced something that was colourful and still not it;
//! the look is in the numbers, the padding and the quiet border, and only
//! then in the hues.
//!
//! Colour is never the only carrier. A tick is `[x]` or `[ ]`, the cursor is
//! a bar in the margin, and a group has a heading: read this screen in
//! monochrome and nothing is lost but the pleasure.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Padding, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::ui::title_case;

/// One line of the plugin list.
pub struct Choice {
    pub label: String,
    pub blurb: String,
    pub on: bool,
    /// The choice this one is part of, by index, for a choice that is part
    /// of one.
    ///
    /// A child is shown, reachable and counted only while its parent is
    /// ticked. Hidden rather than disabled: a row you can land on but not
    /// change is a row that looks broken, and the reason it will not change
    /// is off screen.
    pub under: Option<usize>,
    /// Choices this one cannot be taken without, by index.
    ///
    /// Ticking this row ticks them, and unticking one of them unticks this
    /// row. The screen may do that where a flag may not, because every row
    /// it changed is visible and can be changed back.
    pub needs: Vec<usize>,
    /// Why this choice cannot be ticked, for a choice that cannot.
    ///
    /// Drawn and not hidden, which is the opposite of what [`Choice::under`]
    /// does with a child, and for the reason stated there: a row you cannot
    /// change looks broken when the reason is off screen. Here the reason is
    /// the row's own blurb, so it is the first thing read. The cursor steps
    /// over it, so nothing invites a key that would do nothing.
    pub blocked: Option<String>,
}

/// Everything `create` needs, and nothing it does not.
///
/// The same struct the flags produce. The chooser edits it in place and hands
/// it back, so the two routes into the scaffold cannot drift apart.
pub struct Draft {
    pub name: String,
    pub title: String,
    pub choices: Vec<Choice>,
    pub git: bool,
}

impl Draft {
    /// Whether a choice is on the screen at all.
    ///
    /// One level deep, deliberately: nothing here nests twice, and a
    /// recursive answer would be code written for a case that does not
    /// exist.
    pub fn shown(&self, index: usize) -> bool {
        match self.choices[index].under {
            Some(parent) => self.choices[parent].on,
            None => true,
        }
    }

    /// Whether a choice is part of the answer. A ticked child of an unticked
    /// parent is not: it is a tick nobody can see.
    pub fn taken(&self, index: usize) -> bool {
        self.choices[index].on && self.shown(index) && !self.blocked(index)
    }

    /// Whether a choice is on the screen and cannot be ticked.
    pub fn blocked(&self, index: usize) -> bool {
        self.choices[index].blocked.is_some()
    }
}

/// Where the cursor is. Derived from the draft each frame rather than stored,
/// so the list length and the row set can never disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Name,
    Title,
    Choice(usize),
    Git,
}

struct Model {
    draft: Draft,
    row: usize,
    /// Until the title is edited by hand it mirrors the name. After that it is
    /// the user's, and typing in the name must stop overwriting it.
    title_is_mine: bool,
    /// Set when Enter is pressed on an invalid name, so the reason appears
    /// without having to also appear while the field is still being typed.
    complained: bool,
}

impl Model {
    fn rows(&self) -> Vec<Row> {
        let mut rows = vec![Row::Name, Row::Title];
        rows.extend(
            (0..self.draft.choices.len())
                .filter(|i| self.draft.shown(*i))
                .map(Row::Choice),
        );
        rows.push(Row::Git);
        rows
    }

    fn here(&self) -> Row {
        let rows = self.rows();
        rows[self.row.min(rows.len() - 1)]
    }

    /// One row, in the direction asked, over any row that cannot be
    /// changed.
    ///
    /// `by` is the direction as well as the distance, so stepping over a
    /// blocked row keeps going the same way rather than bouncing. The name
    /// and title rows are never blocked, so the walk always terminates.
    fn step(&mut self, by: isize) {
        let rows = self.rows();
        let n = rows.len() as isize;
        let direction = if by < 0 { -1 } else { 1 };
        let wrap = |at: isize| ((at % n) + n) % n;
        let mut at = wrap(self.row as isize + by);
        for _ in 0..n {
            match rows[at as usize] {
                Row::Choice(i) if self.draft.blocked(i) => at = wrap(at + direction),
                _ => break,
            }
        }
        self.row = at as usize;
    }

    /// Text goes to the field under the cursor; a space on a tick-box toggles
    /// it. Space has to mean both because a title legitimately contains one.
    fn type_char(&mut self, c: char) {
        match self.here() {
            Row::Name => {
                self.draft.name.push(c);
                self.complained = false;
                if !self.title_is_mine {
                    self.draft.title = title_case(&self.draft.name);
                }
            }
            Row::Title => {
                self.draft.title.push(c);
                self.title_is_mine = true;
            }
            _ if c == ' ' => self.toggle(),
            _ => {}
        }
    }

    fn backspace(&mut self) {
        match self.here() {
            Row::Name => {
                self.draft.name.pop();
                self.complained = false;
                if !self.title_is_mine {
                    self.draft.title = title_case(&self.draft.name);
                }
            }
            Row::Title => {
                self.draft.title.pop();
                self.title_is_mine = true;
            }
            _ => {}
        }
    }

    fn toggle(&mut self) {
        match self.here() {
            Row::Choice(i) => {
                let on = !self.draft.choices[i].on;
                self.draft.choices[i].on = on;
                if on {
                    // What this row needs comes on with it — and so does
                    // what its children need. A child that starts on
                    // arrives the moment its parent is ticked, and it has
                    // requirements of its own: `rsx` brings
                    // `rsx-tailwind`, which cannot be written without
                    // `commands`. Satisfying only the ticked row's needs
                    // produced a selection the flags refuse, which is the
                    // two front doors disagreeing.
                    let mut wants: Vec<usize> = self.draft.choices[i].needs.clone();
                    for at in 0..self.draft.choices.len() {
                        if self.draft.choices[at].under == Some(i) && self.draft.choices[at].on {
                            wants.extend(self.draft.choices[at].needs.iter().copied());
                        }
                    }
                    for at in wants {
                        self.draft.choices[at].on = true;
                    }
                } else {
                    // And a row that needed this one goes off with it,
                    // rather than staying ticked against something that is
                    // not there any more.
                    for at in 0..self.draft.choices.len() {
                        if self.draft.choices[at].needs.contains(&i) {
                            self.draft.choices[at].on = false;
                        }
                    }
                }
            }
            Row::Git => self.draft.git = !self.draft.git,
            _ => {}
        }
    }
}

/// Runs the chooser. `Ok(None)` means the person cancelled.
///
/// `validate` is the caller's own naming rule, passed in rather than copied:
/// the authority on what a package may be called is the command, and a second
/// copy here would be a second answer to the same question.
pub fn choose(
    draft: Draft,
    validate: impl Fn(&str) -> Option<String>,
) -> io::Result<Option<Draft>> {
    let mut terminal = ratatui::try_init()?;
    let outcome = drive(&mut terminal, draft, validate);
    // Restore before returning either way: an error path that leaves the
    // terminal in raw mode is worse than the error.
    ratatui::try_restore()?;
    outcome
}

fn drive(
    terminal: &mut DefaultTerminal,
    draft: Draft,
    validate: impl Fn(&str) -> Option<String>,
) -> io::Result<Option<Draft>> {
    let mut model = Model {
        title_is_mine: !draft.title.is_empty(),
        draft,
        row: 0,
        complained: false,
    };

    loop {
        let problem = validate(&model.draft.name);
        terminal.draw(|frame| view(frame, &model, problem.as_deref()))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        // Without this the handler fires twice per keystroke on Windows, which
        // on a tick-box means it toggles back.
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => return Ok(None),
            KeyCode::Char('c') if ctrl => return Ok(None),
            KeyCode::Enter => {
                if problem.is_none() {
                    return Ok(Some(model.draft));
                }
                model.complained = true;
            }
            KeyCode::Up | KeyCode::BackTab => model.step(-1),
            KeyCode::Down | KeyCode::Tab => model.step(1),
            KeyCode::Backspace => model.backspace(),
            KeyCode::Char(c) => model.type_char(c),
            _ => {}
        }
    }
}

// -- the view ---------------------------------------------------------------

// Charm's palette, by the numbers they publish rather than by eye. Lip
// Gloss's own examples are the reference: #7D56F4 behind #FAFAFA is the
// title block every Bubble Tea tutorial opens with, and #FF5F87 is the
// colour `huh` puts on the row the cursor is on.

/// The brand. A background far more often than a foreground.
pub const PURPLE: Color = Color::Rgb(0x7D, 0x56, 0xF4);
/// Where you are, and what you are changing.
pub const PINK: Color = Color::Rgb(0xFF, 0x5F, 0x87);
/// Headings, and the rule down the side of a group.
pub const FUCHSIA: Color = Color::Rgb(0xEE, 0x6F, 0xF8);
/// On.
pub const GREEN: Color = Color::Rgb(0x43, 0xBF, 0x6D);
/// Text on the brand block. Not white: white on purple glares.
const CREAM: Color = Color::Rgb(0xFA, 0xFA, 0xFA);
/// Secondary text. Charm's help grey, and it is darker than feels right
/// until you see it next to the rest.
const GREY: Color = Color::Rgb(0x62, 0x62, 0x62);
/// A step up from the help grey, for text that is secondary and still read.
const SILVER: Color = Color::Rgb(0x9B, 0x9B, 0x9B);

/// The accent, kept under its old name for the rows that act on it.
const ACCENT: Color = PINK;

/// Secondary text.
///
/// A colour rather than the DIM modifier: terminals disagree about how far
/// dim goes, and a few drop it, so a grey that is actually grey keeps the
/// three tones apart everywhere.
fn dim() -> Style {
    Style::new().fg(SILVER)
}

/// Tertiary: present, and not to be read unless looked for.
fn faint() -> Style {
    Style::new().fg(GREY)
}

fn view(frame: &mut Frame, model: &Model, problem: Option<&str>) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .padding(Padding::new(2, 2, 1, 1))
        .border_style(Style::new().fg(GREY));
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());

    // Body, then a blank, then one footer row pinned to the bottom.
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let mut lines: Vec<Line> = Vec::new();
    let here = model.here();

    // The title block. Filled, padded, on its own line: the shape every Lip
    // Gloss example opens with, and the reason is that a background says
    // "this is the thing" in a way a bold foreground cannot.
    lines.push(Line::from(Span::styled(
        "  brocanth create  ",
        Style::new()
            .bg(PURPLE)
            .fg(CREAM)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::raw(""));

    lines.push(field("Directory", &model.draft.name, here == Row::Name));
    if let Some(problem) = problem.filter(|_| model.complained) {
        lines.push(Line::from(vec![
            Span::raw("            "),
            Span::styled(problem.to_string(), Style::new().fg(Color::Red)),
        ]));
    }
    lines.push(field("Name", &model.draft.title, here == Row::Title));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  what goes in the binary",
        Style::new().fg(FUCHSIA).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        "  nothing unticked is compiled, linked, or in the lock file",
        faint(),
    )));
    // The column the blurbs start in, measured rather than fixed, so a
    // module with a long name does not shear its own row out of line.
    // Indent included: a child is two columns in, and the blurbs still have
    // to agree across depths.
    let column = model
        .draft
        .choices
        .iter()
        .enumerate()
        .filter(|(i, _)| model.draft.shown(*i))
        .map(|(_, choice)| choice.label.chars().count() + 2 * usize::from(choice.under.is_some()))
        .chain(std::iter::once("git".len()))
        .max()
        .unwrap_or(0)
        + 1;
    // The group a row belongs to, so a heading is written once, above the
    // first row of each group, and not again.
    let mut group: Option<usize> = None;
    for (i, choice) in model.draft.choices.iter().enumerate() {
        if !model.draft.shown(i) {
            continue;
        }
        if choice.under != group {
            if let Some(parent) = choice.under {
                lines.push(subsection(&model.draft.choices[parent].label));
            }
            group = choice.under;
        }
        // The reason replaces the gist rather than following it. What the
        // row has to answer is why it is not available, and a row that
        // answers two questions answers neither at this width.
        let (blurb, state) = match &choice.blocked {
            Some(why) => (why.as_str(), Box::Blocked),
            None if choice.on => (choice.blurb.as_str(), Box::On),
            None => (choice.blurb.as_str(), Box::Off),
        };
        lines.push(nested(
            &choice.label,
            Some(blurb),
            state,
            here == Row::Choice(i),
            usize::from(choice.under.is_some()),
            column,
        ));
    }

    lines.push(Line::raw(""));
    lines.push(tick(
        "git",
        Some("initialise a repository"),
        model.draft.git,
        here == Row::Git,
        column,
    ));

    frame.render_widget(Paragraph::new(lines), body);

    let second = if matches!(here, Row::Name | Row::Title) {
        ("type", "to edit")
    } else {
        ("space", "toggle")
    };
    let keys = [
        ("\u{2191}\u{2193}", "move"),
        second,
        ("enter", "create"),
        ("esc", "cancel"),
    ];
    let mut help = vec![Span::raw("  ")];
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            help.push(Span::styled(" \u{2022} ", faint()));
        }
        help.push(Span::styled(*key, Style::new().fg(SILVER)));
        help.push(Span::styled(format!(" {what}"), faint()));
    }
    frame.render_widget(Paragraph::new(Line::from(help)), footer);
}

/// A labelled text field. The cursor is a reversed cell on the active row, so
/// it needs no cursor positioning and survives a resize.
fn field<'a>(label: &'a str, value: &'a str, active: bool) -> Line<'a> {
    let mut spans = vec![
        Span::styled(
            if active { "\u{2502} " } else { "  " },
            Style::new().fg(PINK),
        ),
        Span::styled(format!("{label:<9} "), if active { dim() } else { faint() }),
        Span::styled(
            value,
            if active {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            },
        ),
    ];
    if active {
        // The caret. A filled cell rather than a terminal cursor, so it
        // needs no positioning and survives a resize.
        spans.push(Span::styled(" ", Style::new().bg(PINK)));
    }
    Line::from(spans)
}

/// The heading over the rows that are part of the row above them.
///
/// Indentation alone is weak once a list is long: two steps of two columns
/// read as a wrapped line. The heading names the group in the same dim voice
/// as the heading over the whole list, and sits in the tick-box column, so
/// the group has a top edge as well as an indent.
fn subsection(parent: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled("    \u{2570}\u{2500} ", Style::new().fg(FUCHSIA)),
        Span::styled(format!("inside {parent}"), Style::new().fg(FUCHSIA)),
    ])
}

/// What is in the box at the start of a row.
///
/// Three states and not a `bool`, because the third is not "off": a row
/// nobody can tick is a different thing from a row nobody has ticked, and
/// the glyph has to say which without colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Box {
    On,
    Off,
    Blocked,
}

impl Box {
    /// Spelled rather than coloured, so the state survives a monochrome
    /// terminal and colour-blindness both.
    fn glyph(self) -> &'static str {
        match self {
            Box::On => "[\u{2713}] ",
            Box::Off => "[ ] ",
            Box::Blocked => "[\u{2013}] ",
        }
    }
}

/// A tick-box.
fn tick<'a>(
    label: &'a str,
    blurb: Option<&'a str>,
    on: bool,
    active: bool,
    column: usize,
) -> Line<'a> {
    nested(
        label,
        blurb,
        if on { Box::On } else { Box::Off },
        active,
        0,
        column,
    )
}

/// The same row, `depth` levels in.
///
/// The cursor marker stays in the leftmost column at every depth, so the
/// eye follows one line down the screen rather than stepping in and out,
/// and the label's own padding shrinks by the indent so the blurbs still
/// align. `column` is where the blurbs start, counted from the label.
fn nested<'a>(
    label: &'a str,
    blurb: Option<&'a str>,
    state: Box,
    active: bool,
    depth: usize,
    column: usize,
) -> Line<'a> {
    let pad = column.saturating_sub(2 * depth);
    let mut spans = vec![
        Span::styled(
            if active { "\u{2502} " } else { "  " },
            Style::new().fg(PINK),
        ),
        Span::raw("  ".repeat(depth)),
        Span::styled(
            state.glyph(),
            match state {
                Box::On => Style::new().fg(GREEN).add_modifier(Modifier::BOLD),
                Box::Off | Box::Blocked => faint(),
            },
        ),
        Span::styled(
            format!("{label:<pad$}"),
            match (active, state) {
                (true, _) => Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
                // Lit when it is in, plain when it is not, so the list can be
                // read down the left edge without reading the boxes.
                (false, Box::On) => Style::new(),
                (false, Box::Off) => dim(),
                // Fainter than unticked: it is not a thing to decide about.
                (false, Box::Blocked) => faint(),
            },
        ),
    ];
    if let Some(blurb) = blurb {
        spans.push(Span::styled(format!(" {blurb}"), dim()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A parent with one child, for the nesting tests.
    fn nested_model() -> Model {
        let mut m = model();
        m.draft.choices[1].under = Some(0);
        m.draft.choices[1].on = true;
        m
    }

    #[test]
    fn a_child_is_off_the_screen_while_its_parent_is_unticked() {
        let mut m = nested_model();
        assert_eq!(m.rows().len(), 5, "name, title, parent, child, git");
        m.draft.choices[0].on = false;
        assert!(
            !m.rows().contains(&Row::Choice(1)),
            "a row nobody can change must not be a row"
        );
    }

    #[test]
    fn a_tick_nobody_can_see_is_not_part_of_the_answer() {
        // The child stays ticked so that re-ticking the parent restores what
        // was there, but while the parent is off it is not an answer.
        let mut m = nested_model();
        assert!(m.draft.taken(1));
        m.draft.choices[0].on = false;
        assert!(m.draft.choices[1].on, "the tick is remembered");
        assert!(!m.draft.taken(1), "but it is not taken");
    }

    #[test]
    fn the_cursor_cannot_land_on_a_hidden_child() {
        let mut m = nested_model();
        m.draft.choices[0].on = false;
        for _ in 0..12 {
            m.step(1);
            assert_ne!(m.here(), Row::Choice(1));
        }
    }

    #[test]
    fn a_group_has_one_heading_and_it_leaves_with_the_group() {
        let mut m = nested_model();
        let screen = render(&m, None, 72, 18);
        assert_eq!(
            screen.matches("inside config").count(),
            1,
            "one heading over the group, not one per row: {screen}"
        );
        m.draft.choices[0].on = false;
        let screen = render(&m, None, 72, 18);
        assert!(
            !screen.contains("inside config"),
            "a heading over no rows: {screen}"
        );
    }

    /// A parent with a child nobody can tick.
    fn blocked_model() -> Model {
        let mut m = nested_model();
        m.draft.choices[1].blocked = Some("the core's response body is finished".into());
        m
    }

    #[test]
    fn a_row_nobody_can_tick_is_on_the_screen_with_the_reason_on_it() {
        // Hidden is what a child of an unticked parent gets, and it is the
        // wrong answer here: "where is SSE" is a question somebody asks
        // once a month until the menu answers it.
        let screen = render(&blocked_model(), None, 72, 16);
        assert!(screen.contains("[\u{2013}] model"), "{screen}");
        assert!(
            screen.contains("the core's response body is finished"),
            "the row does not say why:\n{screen}"
        );
    }

    #[test]
    fn the_cursor_steps_over_a_row_it_cannot_change() {
        // A row you can land on but not change looks broken.
        let mut m = blocked_model();
        m.row = 2; // the parent
        m.step(1);
        assert_eq!(m.here(), Row::Git, "it landed on the blocked child");
        m.step(-1);
        assert_eq!(m.here(), Row::Choice(0), "going back, it landed on it");
    }

    #[test]
    fn a_tick_nobody_could_have_made_is_not_part_of_the_answer() {
        // The flags and the chooser share `Draft`, so a caller that set
        // `on` by mistake must not get the module anyway.
        let mut m = blocked_model();
        m.draft.choices[1].on = true;
        assert!(!m.draft.taken(1));
    }

    fn model() -> Model {
        Model {
            draft: Draft {
                name: String::new(),
                title: String::new(),
                choices: vec![
                    Choice {
                        label: "config".into(),
                        blurb: String::new(),
                        on: true,
                        under: None,
                        blocked: None,
                        needs: Vec::new(),
                    },
                    Choice {
                        label: "model".into(),
                        blurb: String::new(),
                        on: false,
                        under: None,
                        blocked: None,
                        needs: Vec::new(),
                    },
                ],
                git: false,
            },
            row: 0,
            title_is_mine: false,
            complained: false,
        }
    }

    #[test]
    fn the_title_mirrors_the_name_until_the_title_is_touched() {
        let mut m = model();
        for c in "deploy-view".chars() {
            m.type_char(c);
        }
        assert_eq!(m.draft.title, "Deploy View");

        // Move to the title and edit it: the mirror must stop.
        m.step(1);
        m.type_char('!');
        assert_eq!(m.draft.title, "Deploy View!");
        m.step(-1);
        m.type_char('s');
        assert_eq!(m.draft.name, "deploy-views");
        assert_eq!(m.draft.title, "Deploy View!", "the mirror kept writing");
    }

    #[test]
    fn space_types_in_a_field_and_toggles_on_a_tick_box() {
        let mut m = model();
        m.step(1); // Title
        m.type_char('a');
        m.type_char(' ');
        m.type_char('b');
        assert_eq!(m.draft.title, "a b", "space must reach a text field");

        m.step(1); // first choice
        assert!(m.draft.choices[0].on);
        m.type_char(' ');
        assert!(!m.draft.choices[0].on, "space must toggle a tick-box");
    }

    #[test]
    fn the_cursor_wraps_in_both_directions() {
        let mut m = model();
        let last = m.rows().len() - 1;
        m.step(-1);
        assert_eq!(m.row, last, "up from the first row reaches the last");
        m.step(1);
        assert_eq!(m.row, 0, "and down from the last comes back");
    }

    /// Renders a frame into memory and returns it as text.
    ///
    /// The chooser is the one surface in this crate a pipe can never see, so
    /// it is the one most able to rot unnoticed. ratatui's own in-memory
    /// backend gives it the same treatment every other renderer here gets:
    /// draw it, read the bytes back, assert on them. No pseudo-terminal, and
    /// it runs in microseconds.
    fn render(model: &Model, problem: Option<&str>, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test backend");
        terminal
            .draw(|frame| view(frame, model, problem))
            .expect("draw");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer.cell((x, y)).map_or(" ", |c| c.symbol()))
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Prints the chooser as it would appear. Ignored by default because it
    /// asserts nothing; it exists so the layout can be eyeballed without a
    /// terminal:
    ///
    /// ```text
    /// cargo test -p brocanth-cli --bin brocanth -- --ignored --nocapture look
    /// ```
    /// The real registry, as the chooser would show it, with the parent of
    /// each subsection ticked so every child is on screen.
    ///
    /// ```text
    /// cargo test -p brocanth-cli --bin brocanth -- --ignored --nocapture menu
    /// ```
    #[test]
    #[ignore = "prints a frame, asserts nothing"]
    fn menu() {
        let keys = crate::modules::keys();
        let choices = keys
            .iter()
            .map(|key| {
                let m = crate::modules::find(key).expect("registered").manifest();
                Choice {
                    label: m.key.to_string(),
                    blurb: m.gist.to_string(),
                    on: m.parent.is_none(),
                    under: m
                        .parent
                        .and_then(|parent| keys.iter().position(|k| *k == parent)),
                    blocked: m.blocked.map(str::to_string),
                    needs: m
                        .needs
                        .iter()
                        .filter_map(|(need, _)| keys.iter().position(|k| k == need))
                        .collect(),
                }
            })
            .collect();
        let m = Model {
            draft: Draft {
                name: "deploy-view".into(),
                title: "Deploy View".into(),
                choices,
                git: true,
            },
            row: 2,
            title_is_mine: false,
            complained: false,
        };
        println!("\n{}\n", render(&m, None, 112, 30));
    }

    #[test]
    #[ignore = "prints a frame, asserts nothing"]
    fn look() {
        let mut m = model();
        for c in "deploy-view".chars() {
            m.type_char(c);
        }
        m.row = 3;
        println!("\n{}\n", render(&m, None, 72, 15));
    }

    #[test]
    fn the_chooser_draws_every_answer_on_one_screen() {
        let mut m = model();
        for c in "deploy-view".chars() {
            m.type_char(c);
        }
        let screen = render(&m, None, 72, 16);

        // Both text fields, with the mirrored title.
        assert!(screen.contains("Directory"), "{screen}");
        assert!(screen.contains("deploy-view"), "{screen}");
        assert!(screen.contains("Deploy View"), "{screen}");
        // Both tick states, spelled rather than coloured.
        assert!(screen.contains("[\u{2713}] config"), "{screen}");
        assert!(screen.contains("[ ] model"), "{screen}");
        // The cursor, and the keys that move it. A rule against the row,
        // which is a glyph and not a colour, so it is here in the text.
        assert!(screen.contains('\u{2502}'), "{screen}");
        assert!(screen.contains("esc cancel"), "{screen}");
    }

    #[test]
    fn the_reason_a_name_is_refused_appears_only_once_enter_was_pressed() {
        let mut m = model();
        m.type_char('9'); // a package name may not start with a digit

        let quiet = render(&m, Some("does not start with a letter"), 72, 16);
        assert!(
            !quiet.contains("does not start with a letter"),
            "it complained while the field was still being typed:\n{quiet}"
        );

        m.complained = true;
        let loud = render(&m, Some("does not start with a letter"), 72, 16);
        assert!(loud.contains("does not start with a letter"), "{loud}");
    }

    #[test]
    fn the_footer_names_space_only_where_space_toggles() {
        let mut m = model();
        assert!(
            render(&m, None, 72, 16).contains("type to edit"),
            "on a text field the footer must not offer space as a toggle"
        );
        m.row = 2; // a tick-box
        assert!(render(&m, None, 72, 16).contains("space toggle"));
    }

    #[test]
    fn backspace_on_a_tick_box_changes_nothing() {
        let mut m = model();
        m.row = 2; // a choice
        let before = m.draft.choices[0].on;
        m.backspace();
        assert_eq!(m.draft.choices[0].on, before);
        assert!(m.draft.name.is_empty());
    }
}

#[cfg(test)]
mod needs_tests {
    use super::*;

    /// Ticking a parent brings a child that starts on. Does it bring what
    /// that child needs?
    #[test]
    fn a_child_that_arrives_with_its_parent_brings_its_own_requirements() {
        let keys = crate::modules::keys();
        let choices: Vec<Choice> = keys
            .iter()
            .map(|key| {
                let m = crate::modules::find(key).expect("registered").manifest();
                Choice {
                    label: m.key.to_string(),
                    blurb: m.gist.to_string(),
                    on: m.on_with_parent,
                    under: m
                        .parent
                        .and_then(|parent| keys.iter().position(|k| *k == parent)),
                    blocked: m.blocked.map(str::to_string),
                    needs: m
                        .needs
                        .iter()
                        .filter_map(|(need, _)| keys.iter().position(|k| k == need))
                        .collect(),
                }
            })
            .collect();
        let mut model = Model {
            draft: Draft {
                name: "x".into(),
                title: "X".into(),
                choices,
                git: false,
            },
            row: 0,
            title_is_mine: true,
            complained: false,
        };

        // Land on `rsx` and tick it. `rsx-tailwind` starts on and needs
        // `commands`.
        let rsx = model
            .rows()
            .iter()
            .position(|r| matches!(r, Row::Choice(i) if model.draft.choices[*i].label == "rsx"))
            .expect("rsx is on the menu");
        model.row = rsx;
        model.toggle();

        let taken: Vec<&str> = (0..model.draft.choices.len())
            .filter(|i| model.draft.taken(*i))
            .map(|i| model.draft.choices[i].label.as_str())
            .collect();

        assert!(
            taken.contains(&"rsx-tailwind"),
            "the child arrived: {taken:?}"
        );
        assert!(
            taken.contains(&"commands"),
            "and so did what it needs, or the chooser produces an answer the \
             flags would refuse: {taken:?}"
        );
    }
}
