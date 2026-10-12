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
//! The colour vocabulary is [`crate::ui`]'s, spelled in ratatui's types: one
//! accent (cyan), dim for secondary, and the three states. A second accent
//! would mean the first had stopped meaning anything.

use std::io;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::ui::title_case;

/// One line of the plugin list.
pub struct Choice {
    pub label: String,
    pub blurb: String,
    pub on: bool,
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

/// Where the cursor is. Derived from the draft each frame rather than stored,
/// so the list length and the row set can never disagree.
#[derive(Clone, Copy, PartialEq, Eq)]
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
        rows.extend((0..self.draft.choices.len()).map(Row::Choice));
        rows.push(Row::Git);
        rows
    }

    fn here(&self) -> Row {
        let rows = self.rows();
        rows[self.row.min(rows.len() - 1)]
    }

    fn step(&mut self, by: isize) {
        let n = self.rows().len() as isize;
        self.row = (((self.row as isize + by) % n + n) % n) as usize;
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
            Row::Choice(i) => self.draft.choices[i].on = !self.draft.choices[i].on,
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

const ACCENT: Color = Color::Cyan;

fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

fn view(frame: &mut Frame, model: &Model, problem: Option<&str>) {
    let block = Block::bordered()
        .title(Span::styled(
            " brocanth create ",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .border_style(dim());
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());

    // Body, then a blank, then one footer row pinned to the bottom.
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(inner);

    let mut lines: Vec<Line> = Vec::new();
    let here = model.here();

    lines.push(field("Directory", &model.draft.name, here == Row::Name));
    if let Some(problem) = problem.filter(|_| model.complained) {
        lines.push(Line::from(vec![
            Span::raw("             "),
            Span::styled(problem.to_string(), Style::new().fg(Color::Red)),
        ]));
    }
    lines.push(field("Name", &model.draft.title, here == Row::Title));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled("  what goes in the binary", dim())));
    lines.push(Line::from(Span::styled(
        "  nothing unticked is compiled, linked, or in the lock file",
        dim(),
    )));
    for (i, choice) in model.draft.choices.iter().enumerate() {
        lines.push(tick(
            &choice.label,
            Some(&choice.blurb),
            choice.on,
            here == Row::Choice(i),
        ));
    }

    lines.push(Line::raw(""));
    lines.push(tick(
        "git",
        Some("initialise a repository"),
        model.draft.git,
        here == Row::Git,
    ));

    frame.render_widget(Paragraph::new(lines), body);

    let keys = if matches!(here, Row::Name | Row::Title) {
        "  \u{2191}\u{2193} move \u{b7} type to edit \u{b7} enter create \u{b7} esc cancel"
    } else {
        "  \u{2191}\u{2193} move \u{b7} space toggle \u{b7} enter create \u{b7} esc cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(keys, dim()))),
        footer,
    );
}

/// A labelled text field. The cursor is a reversed cell on the active row, so
/// it needs no cursor positioning and survives a resize.
fn field<'a>(label: &'a str, value: &'a str, active: bool) -> Line<'a> {
    let mut spans = vec![
        Span::raw(if active { "  > " } else { "    " }),
        Span::styled(format!("{label:<9} "), dim()),
        Span::styled(
            value,
            if active {
                Style::new().fg(ACCENT)
            } else {
                Style::new()
            },
        ),
    ];
    if active {
        spans.push(Span::styled(
            " ",
            Style::new().add_modifier(Modifier::REVERSED),
        ));
    }
    Line::from(spans)
}

/// A tick-box. `[x]` and `[ ]` rather than colour alone, so the state survives
/// a monochrome terminal and colour-blindness both.
fn tick<'a>(label: &'a str, blurb: Option<&'a str>, on: bool, active: bool) -> Line<'a> {
    let mut spans = vec![
        Span::raw(if active { "  > " } else { "    " }),
        Span::styled(
            if on { "[x] " } else { "[ ] " },
            if on {
                Style::new().fg(Color::Green)
            } else {
                dim()
            },
        ),
        Span::styled(
            format!("{label:<10}"),
            if active {
                Style::new().fg(ACCENT)
            } else {
                Style::new()
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
                    },
                    Choice {
                        label: "model".into(),
                        blurb: String::new(),
                        on: false,
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
        assert!(screen.contains("[x] config"), "{screen}");
        assert!(screen.contains("[ ] model"), "{screen}");
        // The cursor, and the keys that move it.
        assert!(screen.contains("  > "), "{screen}");
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
