//! The chooser: two views over one answer.
//!
//! Reached from `brocanth create` when [`crate::ui::Face::Tui`] says both
//! descriptors are a terminal *and* at least one answer is missing. With every
//! answer supplied as a flag this module is never entered, and on anything
//! less than the top rung it is never reachable -- a missing answer there is a
//! diagnostic naming the flag, because a prompt in CI blocks until it times
//! out.
//!
//! # Two tabs, one answer
//!
//! **Wizard** asks one question per screen. It opens with a shape -- an HTML
//! application, a JSON API, something that does work -- which fills in an
//! answer you then edit, because nineteen empty tick-boxes is a worse first
//! screen than a wrong guess you can see. The shape also decides how many
//! questions follow: see [`crate::wizard`].
//!
//! **Expert** draws every row at once, nested, which is what this chooser was
//! before the wizard existed. It is also the review screen: a list of every
//! module with its tick already says what was chosen and what was not.
//!
//! They edit one [`Draft`]. Two structures would drift, and the first symptom
//! would be a tab that shows something the other cannot produce.
//!
//! This shape was designed somewhere else. A throwaway in Go, with Bubble
//! Tea, printed the summary and generated nothing, so an experiment cost a
//! keystroke rather than a rebuild and ten blessed snapshots. What arrived
//! here is the result, down to the row spacing -- and one bug it found
//! first: ticking `rsx` gave `rsx-tailwind` without `commands`, which is a
//! selection the flags refuse.
//!
//! # Three decisions worth defending
//!
//! **The chooser can set nothing a flag cannot.** It fills the same [`Draft`]
//! the flags fill, and returns it. There is no answer reachable only by a
//! person with a terminal -- otherwise a scaffold would exist that CI could
//! not reproduce.
//!
//! **A tick may bring what it requires, and a flag may not.** `rsx-tailwind`
//! writes a subcommand and cannot be had without `commands`. Here ticking one
//! ticks the other and says so in a note under the list, because every row it
//! changed is still on the screen. A flag that quietly added a module would be
//! a flag nobody can read, so the flat command line refuses instead and names
//! the fix.
//!
//! **Cancelling is an answer.** Escape returns `None`, which the caller must
//! tell apart from "accepted every default". Writing an application because
//! someone pressed Escape is the one unrecoverable mistake available here: the
//! command's next act is to create a directory. The Go sketch quits on `q`,
//! which is safe when the whole output is a paragraph on stdout; this one does
//! not, because the keys are the same and the consequence is not.
//!
//! # The palette
//!
//! The admin's, which is Stripe's documentation as close as a token set gets:
//! navy ink rather than black, one violet accent, rules light enough to read
//! as separation rather than as lines. The dark values, because this screen is
//! only ever reached when both descriptors are a terminal, and a terminal is
//! dark far more often than not.
//!
//! Colour is never the only carrier. A tick is `[x]`, `[ ]` or `[-]`, the
//! cursor is a caret in the margin, a group has a heading, and the active tab
//! has a heavier rule under it: read this screen in monochrome and nothing is
//! lost but the pleasure.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::{DefaultTerminal, Frame};

use crate::ui::title_case;
use crate::wizard::{self, PRESETS, QUESTIONS};

/// One line of the plugin list.
pub struct Choice {
    pub label: String,
    pub blurb: String,
    /// The shorter phrasing the wizard uses, which may be the gist.
    pub short: String,
    /// The wizard question this choice belongs to.
    pub section: String,
    pub on: bool,
    /// Whether this choice is ticked the moment its parent is.
    pub starts_on: bool,
    /// The choice this one is part of, by index, for a choice that is part
    /// of one.
    ///
    /// A child is shown, reachable and counted only while its parent is
    /// ticked. Hidden rather than disabled: a row you can land on but not
    /// change is a row that looks broken, and the reason it will not change
    /// is off screen.
    pub under: Option<usize>,
    /// Choices this one cannot be taken without, by index, each with the
    /// sentence shown when it is the reason something changed under you.
    ///
    /// Ticking this row ticks them, and unticking one of them unticks this
    /// row. The screen may do that where a flag may not, because every row it
    /// changed is visible, says so, and can be changed back.
    pub needs: Vec<(usize, String)>,
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
    /// One level deep, deliberately: nothing here nests twice, and a recursive
    /// answer would be code written for a case that does not exist.
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

    /// How many choices are part of the answer.
    fn count(&self) -> usize {
        (0..self.choices.len()).filter(|i| self.taken(*i)).count()
    }

    fn at(&self, key: &str) -> Option<usize> {
        self.choices.iter().position(|c| c.label == key)
    }
}

/// Which view is in front.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Wizard,
    Expert,
}

impl Tab {
    /// The tabs, in the order the bar draws them. One list, so a third is
    /// one line rather than three places that count to two.
    const ALL: [Tab; 2] = [Tab::Wizard, Tab::Expert];

    fn label(self) -> &'static str {
        match self {
            Tab::Wizard => "Wizard",
            Tab::Expert => "Expert",
        }
    }

    fn turn(self, by: isize) -> Tab {
        let n = Tab::ALL.len() as isize;
        let at = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0) as isize;
        Tab::ALL[(((at + by) % n + n) % n) as usize]
    }
}

/// Where the cursor is in the expert view. Derived from the draft each frame
/// rather than stored, so the list length and the row set can never disagree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Row {
    Name,
    Title,
    Choice(usize),
    Git,
}

struct Model {
    draft: Draft,
    tab: Tab,

    /// Where the cursor is in the expert view.
    row: usize,

    /// Where we are in the wizard's plan -- not which question that is. The
    /// plan is shorter for some shapes, so "4 of 6" and `QUESTIONS[3]` are no
    /// longer the same number.
    at: usize,
    /// Where the cursor is within the question on screen.
    wrow: usize,
    /// The shape, once the first question is answered. `None` before that,
    /// which asks every question: a count that started short and grew would
    /// read as a mistake.
    preset: Option<usize>,

    /// What the last change did beyond what was asked.
    ///
    /// A tick that pulls in a requirement says so; an untick that drops a
    /// dependant says so. Cleared by the next change, so the screen never
    /// explains something that is two actions old.
    notes: Vec<String>,

    /// Until the title is edited by hand it mirrors the name. After that it is
    /// the user's, and typing in the name must stop overwriting it.
    title_is_mine: bool,
    /// Set when Enter is pressed on an invalid name, so the reason appears
    /// without having to also appear while the field is still being typed.
    complained: bool,
}

// -- the answer -------------------------------------------------------------

impl Model {
    /// Turn one choice on or off, and settle everything that follows.
    ///
    /// The note beside each change is the whole argument for letting a screen
    /// do this at all. A flag that silently added a module would be
    /// unreadable; a row that changed under you and said why, on a screen
    /// where you can change it back, is not.
    fn set(&mut self, index: usize, on: bool) {
        if self.draft.blocked(index) {
            return;
        }
        self.notes.clear();
        self.settle(index, on);
    }

    fn settle(&mut self, index: usize, on: bool) {
        self.draft.choices[index].on = on;
        if !on {
            // Anything that needed it goes too, rather than staying ticked
            // against something that is not there. A child of a parent that
            // just went off is left ticked rather than cleared, so putting
            // the parent back restores what was chosen under it: `taken` is
            // what decides the answer.
            for other in 0..self.draft.choices.len() {
                let why = self.draft.choices[other]
                    .needs
                    .iter()
                    .find(|(need, _)| *need == index)
                    .map(|(_, why)| why.clone());
                if let (Some(why), true) = (why, self.draft.choices[other].on) {
                    self.draft.choices[other].on = false;
                    let label = self.draft.choices[other].label.clone();
                    self.notes.push(format!("{label} went with it — {why}"));
                }
            }
            return;
        }

        for (need, why) in self.draft.choices[index].needs.clone() {
            if !self.draft.choices[need].on {
                self.draft.choices[need].on = true;
                let label = self.draft.choices[need].label.clone();
                self.notes.push(format!("{label} came with it — {why}"));
            }
        }
        // A parent arrives with its child, for the same reason. It cannot
        // happen from a keypress -- a child is off the screen while its
        // parent is -- but a shape may name a child, and a tick nobody can
        // see is not an answer.
        if let Some(parent) = self.draft.choices[index].under {
            if !self.draft.choices[parent].on {
                let child = self.draft.choices[index].label.clone();
                let name = self.draft.choices[parent].label.clone();
                self.notes
                    .push(format!("{name} came with it — {child} is part of it"));
                self.settle(parent, true);
            }
        }
        // And a child that starts on arrives with its parent -- through this
        // same path and not by setting the flag, because the child has
        // requirements of its own. `rsx` brings `rsx-tailwind`, which needs
        // `commands`; satisfying only the ticked row's needs produced a
        // selection the flags refuse, which is the two front doors
        // disagreeing. The Go sketch found it.
        for child in 0..self.draft.choices.len() {
            let c = &self.draft.choices[child];
            if c.under == Some(index) && c.starts_on && !c.on && c.blocked.is_none() {
                let name = self.draft.choices[child].label.clone();
                let parent = self.draft.choices[index].label.clone();
                self.notes.push(format!(
                    "{name} is part of {parent} and starts on; untick it if you do not want it"
                ));
                self.settle(child, true);
            }
        }
    }

    /// Take a shape: clear the answer and tick what it names.
    fn take(&mut self, preset: usize) {
        self.preset = Some(preset);
        for choice in &mut self.draft.choices {
            choice.on = false;
        }
        for pick in PRESETS[preset].picks {
            if let Some(index) = self.draft.at(pick) {
                self.settle(index, true);
            }
        }
        // A shape is a written list, and a note under it would only repeat
        // what the row above already said it would do.
        self.notes.clear();
    }
}

// -- the wizard -------------------------------------------------------------

impl Model {
    /// The questions this run asks, as indices into [`QUESTIONS`].
    fn plan(&self) -> Vec<usize> {
        wizard::plan(self.preset)
    }

    /// The question on screen.
    fn question(&self) -> usize {
        let plan = self.plan();
        plan[self.at.min(plan.len() - 1)]
    }

    /// The rows of the question on screen: shapes on the first, choices by
    /// index on the rest.
    ///
    /// A child is asked about only once its parent is taken. The wizard is a
    /// sequence of questions, and a question about something that is not
    /// there is noise.
    fn wizard_rows(&self) -> Vec<usize> {
        let question = self.question();
        if question == 0 {
            return (0..PRESETS.len()).collect();
        }
        // Read off the draft and not the registry, so what the screen shows
        // is what the caller handed over. The two agree today; a test that
        // builds a draft by hand would make them disagree, and the screen
        // must draw the draft.
        let key = QUESTIONS[question].key;
        (0..self.draft.choices.len())
            .filter(|index| wizard::asks_about(key, &self.draft.choices[*index].section))
            .filter(|index| self.draft.shown(*index))
            .collect()
    }

    /// One row, in the direction asked, over any row that cannot be changed.
    fn wizard_step(&mut self, by: isize) {
        let rows = self.wizard_rows();
        let n = rows.len() as isize;
        if n == 0 {
            return;
        }
        let shapes = self.question() == 0;
        let direction = if by < 0 { -1 } else { 1 };
        let wrap = |at: isize| ((at % n) + n) % n;
        let mut at = wrap(self.wrow as isize + by);
        for _ in 0..n {
            if !shapes && self.draft.blocked(rows[at as usize]) {
                at = wrap(at + direction);
            } else {
                break;
            }
        }
        self.wrow = at as usize;
    }

    fn wizard_toggle(&mut self) {
        let rows = self.wizard_rows();
        let Some(&row) = rows.get(self.wrow) else {
            return;
        };
        if self.question() == 0 {
            self.take(row);
        } else {
            self.set(row, !self.draft.choices[row].on);
        }
    }

    fn next_question(&mut self) {
        if self.at + 1 < self.plan().len() {
            self.at += 1;
            self.wrow = 0;
        } else {
            // Past the last question is the whole answer, which is the
            // expert view.
            self.tab = Tab::Expert;
        }
    }

    fn previous_question(&mut self) {
        if self.at > 0 {
            self.at -= 1;
            self.wrow = 0;
        }
    }
}

// -- the expert view --------------------------------------------------------

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

    /// One row, in the direction asked, over any row that cannot be changed.
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

    /// Whether the cursor is in a field where a letter is a letter.
    ///
    /// Space has to mean both a character and a toggle, because a title
    /// legitimately contains one. So does every other letter the footer
    /// offers as a key: `n` is the next question in the wizard and the first
    /// letter of a name here.
    fn typing(&self) -> bool {
        self.tab == Tab::Expert && matches!(self.here(), Row::Name | Row::Title)
    }

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
            Row::Choice(i) => self.set(i, !self.draft.choices[i].on),
            Row::Git => self.draft.git = !self.draft.git,
            _ => {}
        }
    }
}

// -- the keys ---------------------------------------------------------------

impl Model {
    fn walk(&mut self, by: isize) {
        match self.tab {
            Tab::Wizard => self.wizard_step(by),
            Tab::Expert => self.step(by),
        }
    }

    /// A letter. In a text field it is a letter; everywhere else it is a key.
    fn press(&mut self, c: char) {
        if self.typing() {
            self.type_char(c);
            return;
        }
        match (self.tab, c) {
            (Tab::Wizard, ' ') => self.wizard_toggle(),
            (Tab::Expert, ' ') => self.toggle(),
            (Tab::Wizard, 'n' | 'l') => self.next_question(),
            (Tab::Wizard, 'p' | 'h') => self.previous_question(),
            (_, 'j') => self.walk(1),
            (_, 'k') => self.walk(-1),
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
        tab: Tab::Wizard,
        row: 0,
        at: 0,
        wrow: 0,
        preset: None,
        notes: Vec::new(),
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
                // Accept, from either view. The answer is the same one seen
                // twice, so there is no screen that has to be reached first.
                if problem.is_none() {
                    return Ok(Some(model.draft));
                }
                // The field that has to change is in the other view, so the
                // complaint goes where the fix is.
                model.tab = Tab::Expert;
                model.row = 0;
                model.complained = true;
            }
            KeyCode::Tab => model.tab = model.tab.turn(1),
            KeyCode::BackTab => model.tab = model.tab.turn(-1),
            KeyCode::Up => model.walk(-1),
            KeyCode::Down => model.walk(1),
            KeyCode::Right if model.tab == Tab::Wizard => model.next_question(),
            KeyCode::Left if model.tab == Tab::Wizard => model.previous_question(),
            KeyCode::Backspace => model.backspace(),
            KeyCode::Char(c) => model.press(c),
            _ => {}
        }
    }
}

// -- the view ---------------------------------------------------------------

// The admin's palette, which is Stripe's documentation as close as a token
// set gets. The dark values: this screen is only ever reached when both
// descriptors are a terminal, and a terminal is dark far more often than not.

/// Body text.
const INK: Color = Color::Rgb(0xE6, 0xEB, 0xF1);
/// Secondary text that is still read.
const INK_SOFT: Color = Color::Rgb(0x9A, 0xA8, 0xBD);
/// Tertiary: present, and not to be read unless looked for.
const INK_FAINT: Color = Color::Rgb(0x6B, 0x77, 0x85);
/// The one accent: the cursor, a tick, the active tab, a note.
const ACCENT: Color = Color::Rgb(0x9A, 0x94, 0xFF);
/// A rule light enough to read as separation rather than as a line.
const LINE: Color = Color::Rgb(0x2A, 0x33, 0x44);
/// A refusal.
const DANGER: Color = Color::Rgb(0xFF, 0x8A, 0x80);

fn ink() -> Style {
    Style::new().fg(INK)
}

fn soft() -> Style {
    Style::new().fg(INK_SOFT)
}

/// Tertiary text.
///
/// A colour rather than the DIM modifier: terminals disagree about how far
/// dim goes, and a few drop it, so a grey that is actually grey keeps the
/// three tones apart everywhere.
fn faint() -> Style {
    Style::new().fg(INK_FAINT)
}

fn bold() -> Style {
    Style::new().fg(INK).add_modifier(Modifier::BOLD)
}

fn accent() -> Style {
    Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
}

fn view(frame: &mut Frame, model: &Model, problem: Option<&str>) {
    let width = frame.area().width as usize;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("brocanth create ", bold()),
        Span::styled(
            if model.draft.name.is_empty() {
                "...".to_string()
            } else {
                model.draft.name.clone()
            },
            faint(),
        ),
    ]));
    lines.extend(tabs(model.tab, width));
    lines.push(Line::raw(""));

    match model.tab {
        Tab::Wizard => wizard_view(&mut lines, model),
        Tab::Expert => expert_view(&mut lines, model, problem),
    }

    for note in &model.notes {
        lines.push(Line::raw(""));
        // Wrapped by hand, and only here. A row of the table that runs past
        // the edge is a blurb cut short, which is a loss worth taking to
        // keep the columns. A note is a sentence, and half of one explains
        // nothing.
        for (i, run) in fold(note, width.saturating_sub(2)).into_iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(if i == 0 { "▌ " } else { "  " }, accent()),
                Span::styled(run, Style::new().fg(ACCENT)),
            ]));
        }
    }

    lines.push(Line::raw(""));
    lines.push(help(model));

    frame.render_widget(Paragraph::new(lines), frame.area());
}

/// Breaks a sentence into lines no wider than `at`, on spaces.
///
/// A word longer than the width goes on its own line and runs over, which
/// is what a path or a crate name does and is better than cutting it.
fn fold(text: &str, at: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line = word.to_string();
        } else if line.chars().count() + 1 + word.chars().count() <= at {
            line.push(' ');
            line.push_str(word);
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The tab bar: a word with a rule under it, the way the admin draws one.
///
/// The active tab's rule is heavier, which is the second carrier: the accent
/// says which one in colour, the weight says it again without.
fn tabs(active: Tab, width: usize) -> Vec<Line<'static>> {
    let mut words: Vec<Span> = Vec::new();
    let mut rule: Vec<Span> = Vec::new();
    let mut used = 0;
    for tab in Tab::ALL {
        let text = format!("  {}  ", tab.label());
        let span = text.chars().count();
        used += span;
        let (style, mark, mark_style) = if tab == active {
            (accent(), "━", Style::new().fg(ACCENT))
        } else {
            (soft(), "─", Style::new().fg(LINE))
        };
        rule.push(Span::styled(mark.repeat(span), mark_style));
        words.push(Span::styled(text, style));
    }
    rule.push(Span::styled(
        "─".repeat(width.saturating_sub(used)),
        Style::new().fg(LINE),
    ));
    vec![Line::from(words), Line::from(rule)]
}

fn help(model: &Model) -> Line<'static> {
    let keys: Vec<(&str, &str)> = match model.tab {
        Tab::Wizard => vec![
            ("space", "choose"),
            ("n/p", "question"),
            ("tab", "expert view"),
            ("enter", "create"),
            ("esc", "cancel"),
        ],
        Tab::Expert if model.typing() => vec![
            ("type", "to edit"),
            ("\u{2191}\u{2193}", "move"),
            ("tab", "wizard"),
            ("enter", "create"),
            ("esc", "cancel"),
        ],
        Tab::Expert => vec![
            ("space", "toggle"),
            ("\u{2191}\u{2193}", "move"),
            ("tab", "wizard"),
            ("enter", "create"),
            ("esc", "cancel"),
        ],
    };
    let mut spans = Vec::new();
    for (i, (key, what)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" \u{00B7} ", faint()));
        }
        spans.push(Span::styled(key.to_string(), soft()));
        spans.push(Span::styled(format!(" {what}"), faint()));
    }
    Line::from(spans)
}

// -- the wizard's screens ---------------------------------------------------

fn wizard_view(lines: &mut Vec<Line<'static>>, model: &Model) {
    let question = model.question();
    let rows = model.wizard_rows();

    // On the shape screen the counter describes the row under the cursor,
    // not the row that was taken. This is the screen that decides how many
    // questions there are, and moving down a row is how two shapes get
    // compared; a number that only moved after a tick would make the
    // comparison cost a tick, and another to undo it.
    let (total, preview) = if question == 0 {
        let under = wizard::plan(Some(model.wrow.min(PRESETS.len() - 1))).len();
        let taken = model.preset.map(|at| wizard::plan(Some(at)).len());
        (under, taken.is_some_and(|n| n != under))
    } else {
        (model.plan().len(), false)
    };

    let mut heading = vec![Span::styled(
        format!("{} of {total}  {}", model.at + 1, QUESTIONS[question].title),
        bold(),
    )];
    if preview {
        // The cursor is on a shape that is not the taken one and is a
        // different length, so the number is about a choice nobody has made.
        // Say so, rather than letting it read as done.
        heading.push(Span::styled("  if you take this one", faint()));
    }
    lines.push(Line::from(heading));
    lines.push(Line::from(Span::styled(QUESTIONS[question].blurb, soft())));
    lines.push(Line::raw(""));

    if question == 0 {
        for (i, preset) in PRESETS.iter().enumerate() {
            lines.push(Line::from(vec![
                cursor(i == model.wrow),
                Span::styled(
                    if model.preset == Some(i) {
                        "(•) "
                    } else {
                        "( ) "
                    },
                    if model.preset == Some(i) {
                        accent()
                    } else {
                        faint()
                    },
                ),
                Span::styled(preset.name, ink()),
            ]));
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(preset.blurb, faint()),
            ]));
            // What this shape costs in screens. The counter above the list
            // changes when one of these is taken, and a reader who did not
            // see why would read it as a bug.
            let rest = wizard::asked(i);
            lines.push(Line::from(vec![
                Span::raw("      "),
                Span::styled(
                    format!("{} more questions: {}", rest.len(), rest.join(" · ")),
                    faint(),
                ),
            ]));
        }
        return;
    }

    if rows.is_empty() {
        lines.push(Line::from(Span::styled(
            "  Nothing to ask here, given what you have chosen.",
            faint(),
        )));
        return;
    }

    for (i, &index) in rows.iter().enumerate() {
        let choice = &model.draft.choices[index];
        // A child stays indented here as well as in the expert view. The
        // question above says what the subject is; the indent says this row
        // is part of the one over it, which is a different thing.
        let indent = "  ".repeat(usize::from(choice.under.is_some()));
        match &choice.blocked {
            Some(why) => lines.push(Line::from(vec![
                cursor(i == model.wrow),
                Span::styled(Box::Blocked.glyph(), faint()),
                Span::raw(indent),
                Span::styled(format!("{} — {why}", choice.label), faint()),
            ])),
            None => lines.push(Line::from(vec![
                cursor(i == model.wrow),
                box_of(choice.on),
                Span::raw(indent),
                Span::styled(choice.short.clone(), ink()),
            ])),
        }
    }
}

/// The caret in the margin. Two columns at every depth, so the eye follows
/// one line down the screen rather than stepping in and out.
fn cursor(active: bool) -> Span<'static> {
    if active {
        Span::styled("❯ ", accent())
    } else {
        Span::raw("  ")
    }
}

fn box_of(on: bool) -> Span<'static> {
    if on {
        Span::styled(Box::On.glyph(), accent())
    } else {
        Span::styled(Box::Off.glyph(), faint())
    }
}

// -- the expert screen ------------------------------------------------------

fn expert_view(lines: &mut Vec<Line<'static>>, model: &Model, problem: Option<&str>) {
    let here = model.here();
    lines.push(Line::from(Span::styled(
        format!("Everything, at once  {} taken", model.draft.count()),
        bold(),
    )));
    lines.push(Line::from(Span::styled(
        "The same answer the wizard builds. A child appears once its parent is on.",
        soft(),
    )));
    lines.push(Line::raw(""));

    lines.push(field("Directory", &model.draft.name, here == Row::Name));
    if let Some(problem) = problem.filter(|_| model.complained) {
        lines.push(Line::from(vec![
            Span::raw("            "),
            Span::styled(problem.to_string(), Style::new().fg(DANGER)),
        ]));
    }
    lines.push(field("Name", &model.draft.title, here == Row::Title));
    lines.push(Line::raw(""));

    // The column the blurbs start in, measured rather than fixed, so a module
    // with a long name does not shear its own row out of line. Indent
    // included: a child is two columns in, and the blurbs still have to agree
    // across depths.
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
        + 2;

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
        // The reason replaces the gist rather than following it. What the row
        // has to answer is why it is not available, and a row that answers
        // two questions answers neither at this width.
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
    lines.push(nested(
        "git",
        Some("initialise a repository"),
        if model.draft.git { Box::On } else { Box::Off },
        here == Row::Git,
        0,
        column,
    ));
}

/// A labelled text field. The caret is a filled cell on the active row, so it
/// needs no cursor positioning and survives a resize.
fn field(label: &str, value: &str, active: bool) -> Line<'static> {
    let mut spans = vec![
        cursor(active),
        Span::styled(
            format!("{label:<9} "),
            if active { soft() } else { faint() },
        ),
        Span::styled(value.to_string(), if active { accent() } else { ink() }),
    ];
    if active {
        spans.push(Span::styled(" ", Style::new().bg(ACCENT)));
    }
    Line::from(spans)
}

/// The heading over the rows that are part of the row above them.
///
/// Indentation alone is weak once a list is long: two steps of two columns
/// read as a wrapped line. The heading names the group, and sits in the
/// tick-box column, so the group has a top edge as well as an indent.
fn subsection(parent: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("      \u{2570}\u{2500} inside {parent}"),
        faint(),
    ))
}

/// What is in the box at the start of a row.
///
/// Three states and not a `bool`, because the third is not "off": a row
/// nobody can tick is a different thing from a row nobody has ticked, and the
/// glyph has to say which without colour.
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

/// A tick-box row, `depth` levels in.
///
/// The cursor marker stays in the leftmost column at every depth, so the eye
/// follows one line down the screen rather than stepping in and out, and the
/// label's own padding shrinks by the indent so the blurbs still align.
/// `column` is where the blurbs start, counted from the label.
fn nested(
    label: &str,
    blurb: Option<&str>,
    state: Box,
    active: bool,
    depth: usize,
    column: usize,
) -> Line<'static> {
    let pad = column.saturating_sub(2 * depth);
    let mut spans = vec![
        cursor(active),
        Span::styled(
            state.glyph(),
            match state {
                Box::On => accent(),
                Box::Off | Box::Blocked => faint(),
            },
        ),
        Span::raw("  ".repeat(depth)),
        Span::styled(
            format!("{label:<pad$}"),
            match (active, state) {
                (true, _) => accent(),
                // Lit when it is in, plain when it is not, so the list can be
                // read down the left edge without reading the boxes.
                (false, Box::On) => ink(),
                (false, Box::Off) => soft(),
                // Fainter than unticked: it is not a thing to decide about.
                (false, Box::Blocked) => faint(),
            },
        ),
    ];
    if let Some(blurb) = blurb {
        spans.push(Span::styled(format!(" {blurb}"), faint()));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn choice(label: &str, section: &str, on: bool) -> Choice {
        Choice {
            label: label.to_string(),
            blurb: String::new(),
            short: label.to_string(),
            section: section.to_string(),
            on,
            starts_on: false,
            under: None,
            needs: Vec::new(),
            blocked: None,
        }
    }

    fn model() -> Model {
        Model {
            draft: Draft {
                name: String::new(),
                title: String::new(),
                choices: vec![
                    choice("config", "foundations", true),
                    choice("model", "data", false),
                ],
                git: false,
            },
            tab: Tab::Expert,
            row: 0,
            at: 0,
            wrow: 0,
            preset: None,
            notes: Vec::new(),
            title_is_mine: false,
            complained: false,
        }
    }

    /// A parent with one child, for the nesting tests.
    fn nested_model() -> Model {
        let mut m = model();
        m.draft.choices[1].under = Some(0);
        m.draft.choices[1].section = "foundations".into();
        m.draft.choices[1].on = true;
        m
    }

    /// The real registry, as the chooser is given it.
    fn registry(tab: Tab) -> Model {
        let keys = crate::modules::keys();
        let choices = keys
            .iter()
            .map(|key| {
                let m = crate::modules::find(key).expect("registered").manifest();
                Choice {
                    label: m.key.to_string(),
                    blurb: m.gist.to_string(),
                    short: m.phrasing().to_string(),
                    section: m.section.to_string(),
                    on: m.parent.is_none() || m.on_with_parent,
                    starts_on: m.on_with_parent,
                    under: m
                        .parent
                        .and_then(|parent| keys.iter().position(|k| *k == parent)),
                    blocked: m.blocked.map(str::to_string),
                    needs: m
                        .needs
                        .iter()
                        .filter_map(|(need, why)| {
                            keys.iter()
                                .position(|k| k == need)
                                .map(|at| (at, (*why).to_string()))
                        })
                        .collect(),
                }
            })
            .collect();
        Model {
            draft: Draft {
                name: "deploy-view".into(),
                title: "Deploy View".into(),
                choices,
                git: true,
            },
            tab,
            row: 2,
            at: 0,
            wrow: 0,
            preset: None,
            notes: Vec::new(),
            title_is_mine: true,
            complained: false,
        }
    }

    // -- the two tabs -------------------------------------------------------

    #[test]
    fn both_tabs_are_on_the_bar_and_the_active_one_has_the_heavier_rule() {
        let screen = render(&registry(Tab::Wizard), None, 92, 30);
        let bar = screen.lines().nth(1).expect("a tab bar");
        assert!(bar.contains("Wizard") && bar.contains("Expert"), "{bar}");
        let rule = screen.lines().nth(2).expect("a rule");
        // Ten columns of heavy rule under "  Wizard  ", then light.
        assert!(rule.starts_with("━━━━━━━━━━─"), "{rule}");

        let screen = render(&registry(Tab::Expert), None, 92, 30);
        let rule = screen.lines().nth(2).expect("a rule");
        assert!(rule.starts_with("──────────━━━━━━━━━━"), "{rule}");
    }

    #[test]
    fn tab_turns_both_ways_and_comes_back() {
        assert_eq!(Tab::Wizard.turn(1), Tab::Expert);
        assert_eq!(Tab::Expert.turn(1), Tab::Wizard);
        assert_eq!(Tab::Wizard.turn(-1), Tab::Expert);
    }

    // -- the wizard ---------------------------------------------------------

    #[test]
    fn the_wizard_opens_on_the_shape_question_with_nothing_answered() {
        let m = registry(Tab::Wizard);
        assert_eq!(m.preset, None, "the shape question starts unanswered");
        let screen = render(&m, None, 92, 34);
        assert!(screen.contains("1 of 6  Shape"), "{screen}");
        assert!(screen.contains("( ) An HTML application"), "{screen}");
        assert!(
            !screen.contains("(•)"),
            "nothing is taken, so nothing is marked:\n{screen}"
        );
    }

    #[test]
    fn a_shape_says_how_many_questions_it_leads_to() {
        let screen = render(&registry(Tab::Wizard), None, 92, 34);
        assert!(
            screen.contains("5 more questions: Foundations · Data · Pages · Behaviour · Doors"),
            "{screen}"
        );
        assert!(
            screen.contains("4 more questions: Foundations · Data · Behaviour · Doors"),
            "a shape that makes no HTML is not asked how:\n{screen}"
        );
    }

    #[test]
    fn the_counter_follows_the_cursor_on_the_shape_screen() {
        let first = |m: &Model| {
            render(m, None, 92, 34)
                .lines()
                .find(|line| line.contains(" of "))
                .expect("a counter")
                .to_string()
        };
        let mut m = registry(Tab::Wizard);
        for want in ["1 of 6", "1 of 5", "1 of 5", "1 of 6"] {
            assert!(first(&m).contains(want), "{}", first(&m));
            m.wizard_step(1);
        }

        // Take the JSON API, then move back up to the HTML shape. The
        // counter is about the row under the cursor, so it has to say that
        // the five questions are not six yet.
        let mut m = registry(Tab::Wizard);
        m.wizard_step(1);
        m.wizard_toggle();
        assert!(
            !first(&m).contains("if you take this one"),
            "the taken shape is not a preview of itself: {}",
            first(&m)
        );
        m.wizard_step(-1);
        let line = first(&m);
        assert!(
            line.contains("1 of 6") && line.contains("if you take this one"),
            "a shape the cursor is only visiting reads: {line}"
        );

        // Two shapes of the same length: the number did not move, so there
        // is nothing to disown.
        m.wizard_step(1);
        m.wizard_step(1);
        assert!(
            !first(&m).contains("if you take this one"),
            "same length, so no preview to announce: {}",
            first(&m)
        );
    }

    #[test]
    fn a_shape_decides_which_questions_follow() {
        let walk = |preset: usize| {
            let mut m = registry(Tab::Wizard);
            for _ in 0..preset {
                m.wizard_step(1);
            }
            m.wizard_toggle();
            let mut titles = Vec::new();
            loop {
                titles.push(QUESTIONS[m.question()].title);
                if m.at + 1 == m.plan().len() {
                    break;
                }
                m.next_question();
            }
            titles
        };
        let full = vec![
            "Shape",
            "Foundations",
            "Data",
            "Pages",
            "Behaviour",
            "Doors",
        ];
        let short = vec!["Shape", "Foundations", "Data", "Behaviour", "Doors"];
        assert_eq!(walk(0), full, "an HTML application is asked how");
        assert_eq!(walk(1), short, "a JSON API is not");
        assert_eq!(walk(2), short, "and neither is a worker");
        assert_eq!(walk(3), full, "picking nothing still asks everything");
    }

    #[test]
    fn past_the_last_question_is_the_whole_answer() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle(); // the HTML shape
        for _ in 0..5 {
            m.next_question();
        }
        assert_eq!(m.tab, Tab::Wizard, "the last question is still a question");
        m.next_question();
        assert_eq!(m.tab, Tab::Expert);
    }

    #[test]
    fn a_question_draws_its_own_modules_and_no_others() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle(); // the HTML shape, which asks all six
        m.next_question();
        m.next_question();
        let screen = render(&m, None, 92, 24);
        assert!(screen.contains("3 of 6  Data"), "{screen}");
        assert!(screen.contains("An ORM with validations"), "{screen}");
        assert!(
            !screen.contains("A line per request"),
            "foundations belong to the question before:\n{screen}"
        );
    }

    #[test]
    fn a_child_stays_indented_in_the_wizard() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle(); // the HTML shape: rsx and both its children
        for _ in 0..3 {
            m.next_question();
        }
        let screen = render(&m, None, 92, 20);
        assert!(screen.contains("4 of 6  Pages"), "{screen}");
        let tailwind = screen
            .lines()
            .find(|line| line.contains("Tailwind"))
            .expect("the child is on the screen");
        let parent = screen
            .lines()
            .find(|line| line.contains("Templates compiled"))
            .expect("the parent is on the screen");
        let indent = |line: &str| line.find('[').expect("a tick-box");
        assert_eq!(
            tailwind.find('T').unwrap() - indent(tailwind),
            parent.find('T').unwrap() - indent(parent) + 2,
            "a child is two columns in:\n{screen}"
        );
    }

    #[test]
    fn the_wizard_says_what_changed_under_you() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle(); // the HTML shape, then clear its settled notes
        m.preset = None;
        for choice in &mut m.draft.choices {
            choice.on = false;
        }
        // Land on `rsx` in the pages question and tick it.
        m.at = 3;
        m.wrow = 0;
        m.wizard_toggle();
        let screen = render(&m, None, 100, 24);
        assert!(
            screen.contains("rsx-tailwind is part of rsx and starts on"),
            "a row that arrived on its own has to say so:\n{screen}"
        );
        assert!(
            screen.contains("commands came with it"),
            "and so does one that arrived because another needed it:\n{screen}"
        );
    }

    #[test]
    fn a_shape_leaves_no_note_because_its_row_already_said_so() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle();
        assert!(m.notes.is_empty(), "{:?}", m.notes);
    }

    #[test]
    fn a_tick_that_arrives_with_a_parent_brings_its_own_requirements() {
        let mut m = registry(Tab::Expert);
        for choice in &mut m.draft.choices {
            choice.on = false;
        }
        let rsx = m.draft.at("rsx").expect("rsx is on the menu");
        m.set(rsx, true);
        let taken: Vec<&str> = (0..m.draft.choices.len())
            .filter(|i| m.draft.taken(*i))
            .map(|i| m.draft.choices[i].label.as_str())
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

    #[test]
    fn unticking_what_a_row_needed_takes_the_row_with_it() {
        let mut m = registry(Tab::Expert);
        let rsx = m.draft.at("rsx").expect("rsx");
        let commands = m.draft.at("commands").expect("commands");
        m.set(rsx, true);
        assert!(m.draft.taken(m.draft.at("rsx-tailwind").unwrap()));
        m.set(commands, false);
        assert!(
            !m.draft.taken(m.draft.at("rsx-tailwind").unwrap()),
            "a row cannot stay ticked against something that is not there"
        );
        assert!(
            m.notes
                .iter()
                .any(|n| n.contains("rsx-tailwind went with it")),
            "{:?}",
            m.notes
        );
    }

    // -- the expert view ----------------------------------------------------

    #[test]
    fn the_expert_view_counts_what_is_taken() {
        let m = registry(Tab::Expert);
        let taken = m.draft.count();
        let screen = render(&m, None, 112, 34);
        assert!(
            screen.contains(&format!("Everything, at once  {taken} taken")),
            "{screen}"
        );
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
        let screen = render(&m, None, 72, 20);
        assert_eq!(
            screen.matches("inside config").count(),
            1,
            "one heading over the group, not one per row: {screen}"
        );
        m.draft.choices[0].on = false;
        let screen = render(&m, None, 72, 20);
        assert!(
            !screen.contains("inside config"),
            "a heading over no rows: {screen}"
        );
    }

    /// A parent with a child nobody can tick.
    fn blocked_model() -> Model {
        let mut m = nested_model();
        m.draft.choices[1].blocked = Some("the transport is not written yet".into());
        m
    }

    #[test]
    fn a_row_nobody_can_tick_is_on_the_screen_with_the_reason_on_it() {
        // Hidden is what a child of an unticked parent gets, and it is the
        // wrong answer here: "where is SSE" is a question somebody asks once
        // a month until the menu answers it.
        let screen = render(&blocked_model(), None, 72, 20);
        assert!(screen.contains("[\u{2013}]   model"), "{screen}");
        assert!(
            screen.contains("the transport is not written yet"),
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
    fn a_blocked_row_is_skipped_in_the_wizard_too() {
        let mut m = registry(Tab::Wizard);
        m.wizard_toggle(); // the HTML shape
        m.at = 4; // behaviour: jobs, realtime, and two transports nobody can tick
        m.wrow = 0;
        let rows = m.wizard_rows();
        for _ in 0..rows.len() * 2 {
            m.wizard_step(1);
            let at = m.wizard_rows()[m.wrow];
            assert!(
                !m.draft.blocked(at),
                "the cursor landed on `{}`",
                m.draft.choices[at].label
            );
        }
    }

    #[test]
    fn a_tick_nobody_could_have_made_is_not_part_of_the_answer() {
        // The flags and the chooser share `Draft`, so a caller that set `on`
        // by mistake must not get the module anyway.
        let mut m = blocked_model();
        m.draft.choices[1].on = true;
        assert!(!m.draft.taken(1));
    }

    #[test]
    fn the_title_mirrors_the_name_until_the_title_is_touched() {
        let mut m = model();
        for c in "deploy-view".chars() {
            m.press(c);
        }
        assert_eq!(m.draft.title, "Deploy View");

        // Move to the title and edit it: the mirror must stop.
        m.step(1);
        m.press('!');
        assert_eq!(m.draft.title, "Deploy View!");
        m.step(-1);
        m.press('s');
        assert_eq!(m.draft.name, "deploy-views");
        assert_eq!(m.draft.title, "Deploy View!", "the mirror kept writing");
    }

    #[test]
    fn a_letter_is_a_letter_in_a_field_and_a_key_everywhere_else() {
        let mut m = model();
        m.step(1); // Title
        for c in "a b".chars() {
            m.press(c);
        }
        assert_eq!(m.draft.title, "a b", "space must reach a text field");
        // `n` is the next question in the wizard, and the first letter of a
        // name here.
        m.press('n');
        assert_eq!(m.draft.title, "a bn");

        m.step(1); // first choice
        assert!(m.draft.choices[0].on);
        m.press(' ');
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

    #[test]
    fn the_chooser_draws_every_answer_on_one_screen() {
        let mut m = model();
        for c in "deploy-view".chars() {
            m.press(c);
        }
        let screen = render(&m, None, 72, 20);

        // Both text fields, with the mirrored title.
        assert!(screen.contains("Directory"), "{screen}");
        assert!(screen.contains("deploy-view"), "{screen}");
        assert!(screen.contains("Deploy View"), "{screen}");
        // Both tick states, spelled rather than coloured.
        assert!(screen.contains("[\u{2713}] config"), "{screen}");
        assert!(screen.contains("[ ] model"), "{screen}");
        // The cursor, and the keys that move it.
        assert!(screen.contains('\u{276f}'), "{screen}");
        assert!(screen.contains("esc cancel"), "{screen}");
        // And the repository, which only this route offers.
        assert!(screen.contains("git"), "{screen}");
    }

    #[test]
    fn the_reason_a_name_is_refused_appears_only_once_enter_was_pressed() {
        let mut m = model();
        m.press('9'); // a package name may not start with a digit

        let quiet = render(&m, Some("does not start with a letter"), 72, 20);
        assert!(
            !quiet.contains("does not start with a letter"),
            "it complained while the field was still being typed:\n{quiet}"
        );

        m.complained = true;
        let loud = render(&m, Some("does not start with a letter"), 72, 20);
        assert!(loud.contains("does not start with a letter"), "{loud}");
    }

    #[test]
    fn the_footer_names_space_only_where_space_toggles() {
        let mut m = model();
        assert!(
            render(&m, None, 72, 20).contains("type to edit"),
            "on a text field the footer must not offer space as a toggle"
        );
        m.row = 2; // a tick-box
        assert!(render(&m, None, 72, 20).contains("space toggle"));
        m.tab = Tab::Wizard;
        // Wider and taller: the wizard's footer is the longer of the two,
        // and the shape question is four rows of three.
        assert!(render(&m, None, 92, 30).contains("n/p question"));
    }

    #[test]
    fn a_note_is_wrapped_and_a_row_is_not() {
        // A row of the table that runs past the edge is a blurb cut short.
        // A note is a sentence, and half of one explains nothing.
        let mut m = registry(Tab::Expert);
        let commands = m.draft.at("commands").expect("commands");
        m.set(commands, false);
        // Taller than the table, with room to spare. This test is about
        // wrapping, and a height that exactly fits today's menu is a test
        // that fails the next time a module joins it.
        let screen = render(&m, None, 72, 60);
        // Whichever choice the untick took with it, the note names it and
        // the sentence finishes. `commands` carries more than one now, and
        // the panel shows the last.
        assert!(screen.contains("went with it"), "{screen}");
        assert!(
            screen.contains("nowhere to put"),
            "the end of the sentence was cut off:\n{screen}"
        );
        for line in screen.lines() {
            assert!(line.chars().count() <= 72, "{line:?}");
        }
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

    /// Prints the chooser as it would appear. Ignored by default because it
    /// asserts nothing; it exists so the layout can be eyeballed without a
    /// terminal:
    ///
    /// ```text
    /// cargo test -p brocanth-cli --bin brocanth -- --ignored --nocapture menu
    /// ```
    #[test]
    #[ignore = "prints frames, asserts nothing"]
    fn menu() {
        let mut m = registry(Tab::Wizard);
        println!("\n{}\n", render(&m, None, 92, 26));
        m.wizard_step(1);
        m.wizard_toggle();
        println!("\n{}\n", render(&m, None, 92, 26));
        for _ in 0..3 {
            m.next_question();
        }
        println!("\n{}\n", render(&m, None, 92, 14));

        let mut m = registry(Tab::Wizard);
        m.wizard_toggle();
        for _ in 0..3 {
            m.next_question();
        }
        println!("\n{}\n", render(&m, None, 92, 14));

        let mut m = registry(Tab::Expert);
        let commands = m.draft.at("commands").expect("commands");
        m.row = 2;
        m.set(commands, false);
        println!("\n{}\n", render(&m, None, 112, 40));
    }
}
