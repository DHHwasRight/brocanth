//! The guided route's questions, and the shapes that choose between them.
//!
//! The chooser has two views over one answer. The flat one puts every
//! module on the screen at once, which is the right shape for somebody who
//! knows the list. This is the other one: one question per screen, in an
//! order where each answer is smaller than the one before.
//!
//! # Why a shape comes first
//!
//! Nineteen empty tick-boxes is a worse first screen than a wrong guess you
//! can see. Most people starting a project want one of three things, and
//! editing a near-miss is faster than building from nothing.
//!
//! The shape also decides how many questions follow. A JSON API and a
//! worker make no HTML, so neither is asked how: they get five questions,
//! not six. A wizard that asks a question the first answer already settled
//! teaches the reader that the first answer did nothing.
//!
//! # Why the questions are here and the membership is not
//!
//! Which question a module answers is written in the module's own
//! [`Manifest::section`], beside its parent and its gist, because it is
//! knowledge about the module. A module from another repository joins a
//! question with one builder call and no edit to this crate -- the same
//! promise [`crate::modules`] makes about the menu itself.
//!
//! What is written here is the list of questions and their order, which is
//! presentation, and the shapes, which are this CLI's opinion about what
//! people build.
//!
//! [`Manifest::section`]: brocanth_scaffold::Manifest::section

use crate::commands::create::about;

/// One screen of the guided route.
pub struct Question {
    /// What a module's `section` has to say to land here.
    pub key: &'static str,
    pub title: &'static str,
    pub blurb: &'static str,
}

/// The questions, in the order they are asked.
///
/// The first is the shape, which has no modules of its own: it is the
/// question that decides which of the rest are asked. The last is the
/// catch-all, drawn only when some module named a section nobody declared
/// -- a module from another repository, most likely -- so that a module on
/// the menu is never a module the guided route cannot reach.
pub const QUESTIONS: &[Question] = &[
    Question {
        key: "shape",
        title: "Shape",
        blurb: "What this application is. Everything after this is a smaller question.",
    },
    Question {
        key: "foundations",
        title: "Foundations",
        blurb: "The three nobody regrets. Each is still a choice.",
    },
    Question {
        key: "data",
        title: "Data",
        blurb: "Where rows and bytes live.",
    },
    Question {
        key: "pages",
        title: "Pages",
        blurb: "How HTML is made, if any is.",
    },
    Question {
        key: "behaviour",
        title: "Behaviour",
        blurb: "What happens that a request did not ask for.",
    },
    Question {
        key: "doors",
        title: "Doors",
        blurb: "Who gets in, and what they may do.",
    },
    Question {
        key: "operations",
        title: "Operations",
        blurb: "Day 2: what it reports, how it ships, what checks every push.",
    },
    Question {
        key: "extras",
        title: "Extras",
        blurb: "Modules that did not say which question they answer.",
    },
];

/// The key of the catch-all question.
pub const EXTRAS: &str = "extras";

/// A shape: what to tick before the first question, and which questions
/// that makes pointless.
pub struct Preset {
    pub name: &'static str,
    pub blurb: &'static str,
    pub picks: &'static [&'static str],
    /// Questions this shape does not have.
    ///
    /// Written here and not derived from `picks`, because "picks nothing"
    /// and "asks nothing" are different answers: the last shape picks
    /// nothing and must still be asked everything.
    pub skips: &'static [&'static str],
}

/// The shapes, as the first question lists them. The last is the escape
/// hatch, and it is the one that asks the most.
pub const PRESETS: &[Preset] = &[
    Preset {
        name: "An HTML application",
        blurb: "Pages, templates, a database, a session.",
        picks: &[
            "observability",
            "observability-logs",
            "observability-health",
            "observability-metrics",
            "docker",
            "ci",
            "ci-github",
            "config",
            "commands",
            "model",
            "sessions",
            "rsx",
            "rsx-tailwind",
            "rsx-htmx",
        ],
        skips: &[],
    },
    Preset {
        name: "A JSON API",
        blurb: "No templates. A document that describes itself.",
        picks: &[
            "observability",
            "observability-logs",
            "observability-health",
            "observability-metrics",
            "docker",
            "ci",
            "ci-github",
            "config",
            "commands",
            "model",
            "validation",
            "openapi",
        ],
        skips: &["pages"],
    },
    Preset {
        name: "Something that does work",
        blurb: "A queue, a cache, and no front end.",
        picks: &[
            "observability",
            "observability-logs",
            "observability-health",
            "observability-metrics",
            "docker",
            "ci",
            "ci-github",
            "config",
            "commands",
            "model",
            "jobs",
            "cache",
        ],
        skips: &["pages"],
    },
    Preset {
        name: "A static site",
        blurb: "The same pages, compiled to files. No server to run.",
        picks: &[
            "static",
            "ci",
            "ci-github",
            "config",
            "commands",
            "rsx",
            "rsx-tailwind",
            "rsx-htmx",
        ],
        // Each of these asks only about modules that need a server.
        skips: &["data", "behaviour", "doors"],
    },
    Preset {
        name: "Nothing yet",
        blurb: "The core and a router. Add the rest by hand.",
        picks: &[],
        skips: &[],
    },
];

/// Whether a question asks about a module that named this section.
///
/// The catch-all takes anything no question declares, so a module from
/// another repository that named a section this CLI has never heard of is
/// still asked about rather than reachable only from the flat view.
pub fn asks_about(question: &str, section: &str) -> bool {
    if question == EXTRAS {
        section.is_empty() || !QUESTIONS.iter().any(|q| q.key == section)
    } else {
        question == section
    }
}

/// Whether any module on the menu answers this question.
///
/// A question with no modules is not asked. The shape question is the one
/// exception: it has no modules by definition, and it is where the rest are
/// decided.
fn inhabited(question: &str) -> bool {
    if question == "shape" {
        return true;
    }
    crate::modules::keys()
        .into_iter()
        .any(|key| asks_about(question, about(key).section))
}

/// The questions a shape asks, as indices into [`QUESTIONS`].
///
/// Index 0 is always there: it is the question that chose the shape.
/// `preset` is `None` before anything is chosen, which asks everything --
/// a count that starts short and grows would read as a mistake.
pub fn plan(preset: Option<usize>) -> Vec<usize> {
    let skips: &[&str] = preset.map_or(&[], |at| PRESETS[at].skips);
    (0..QUESTIONS.len())
        .filter(|at| !skips.contains(&QUESTIONS[*at].key) && inhabited(QUESTIONS[*at].key))
        .collect()
}

/// The modules a question asks about, by key, in menu order.
///
/// The screen reads the draft it was handed rather than this, so the only
/// caller is the test that asserts every module is asked about once.
#[cfg(test)]
pub fn members(question: &str) -> Vec<&'static str> {
    crate::modules::keys()
        .into_iter()
        .filter(|key| asks_about(question, about(key).section))
        .collect()
}

/// The titles a shape leads to, so its row can say what choosing it costs
/// in screens.
pub fn asked(preset: usize) -> Vec<&'static str> {
    plan(Some(preset))
        .into_iter()
        .skip(1) // the shape question itself
        .map(|at| QUESTIONS[at].title)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_module_is_asked_about_exactly_once() {
        for key in crate::modules::keys() {
            let asked: Vec<&str> = QUESTIONS
                .iter()
                .filter(|q| members(q.key).contains(&key))
                .map(|q| q.key)
                .collect();
            assert_eq!(
                asked.len(),
                1,
                "`{key}` is asked about in {asked:?}; a module the guided route \
                 never reaches is a module only the flat view can choose"
            );
        }
    }

    #[test]
    fn a_built_in_module_names_a_question_that_exists() {
        // The catch-all exists for a module from another repository. A
        // module in this workspace that lands there is a typo.
        for key in crate::modules::keys() {
            let section = about(key).section;
            assert!(
                !section.is_empty(),
                "`{key}` does not say which question it answers"
            );
            assert!(
                QUESTIONS.iter().any(|q| q.key == section),
                "`{key}` answers `{section}`, which no question declares"
            );
        }
    }

    #[test]
    fn a_shape_names_modules_that_exist_and_questions_it_has() {
        let keys = crate::modules::keys();
        for preset in PRESETS {
            for pick in preset.picks {
                assert!(
                    keys.contains(pick),
                    "{} picks `{pick}`, which no module declares",
                    preset.name
                );
            }
            for skip in preset.skips {
                assert!(
                    QUESTIONS.iter().any(|q| q.key == *skip),
                    "{} skips `{skip}`, which no question declares",
                    preset.name
                );
            }
        }
    }

    #[test]
    fn a_shape_leaves_no_requirement_unsatisfied() {
        // The flags refuse a pick whose needs are missing. A shape that
        // produced one would be the chooser opening on an answer its own
        // command line would not accept.
        for preset in PRESETS {
            for pick in preset.picks {
                for (need, _) in about(pick).needs {
                    assert!(
                        preset.picks.contains(&need),
                        "{} picks `{pick}` without `{need}`",
                        preset.name
                    );
                }
            }
        }
    }

    #[test]
    fn a_shape_that_skips_a_question_picks_nothing_from_it() {
        for (at, preset) in PRESETS.iter().enumerate() {
            for skip in preset.skips {
                for key in members(skip) {
                    assert!(
                        !preset.picks.contains(&key),
                        "{} is never asked about {skip} and ticks `{key}` anyway",
                        preset.name
                    );
                }
            }
            // And the count on the row is the count of screens that follow.
            assert_eq!(asked(at).len(), plan(Some(at)).len() - 1);
        }
    }

    #[test]
    fn with_no_shape_taken_every_question_is_still_on_the_table() {
        assert_eq!(plan(None).len(), plan(Some(4)).len());
    }
}
