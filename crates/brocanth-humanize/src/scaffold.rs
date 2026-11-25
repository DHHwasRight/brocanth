//! `humanize` -- what `brocanth create` writes when somebody asks for
//! readable values on a page. Compiled only under the `scaffold` feature.
//!
//! A project gets `src/plugins/humanize/mod.rs` with one function,
//! `of(ctx)`: the helpers in the language of the request. With `i18n`, the
//! language is the locale the `i18n` hook chose, and numbers are grouped by
//! that locale's CLDR formats. Without it, the language is the `locale`
//! option in `src/modules/humanize.rs`.
//!
//! No plugin: the helpers hold no state and read nothing at start, so a
//! registration would be a line that does nothing.

use brocanth_scaffold::snippet::module_ref;
use brocanth_scaffold::{
    AgentsSection, Category, Contribution, Dep, Manifest, Module, Opt, Project, Scaffold, Slot,
};

/// The helpers, as a choice on the menu.
pub struct Humanize;

impl Module for Humanize {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "humanize",
            "readable values for a page: \"3 minutes ago\", 1.5 MB, 2nd, 1,234; no dependency",
        )
        .category(Category::Renderer)
        .contributes::<AgentsSection>(
            Contribution::new(
                "humanize",
                "how a page writes a time, a size or a count for a reader",
            ),
            true,
        )
        .adapts_to(
            "i18n",
            "the phrases follow the request's locale, and numbers group by its CLDR formats",
        )
        .option(Opt::text(
            "locale",
            "en",
            "The language of the phrases without the i18n module: en, fr, de or es.",
        ))
        .asked_in(
            "pages",
            "Readable times, sizes and numbers: \"3 minutes ago\"",
        )
        .krate(
            Dep::brocanth("brocanth-humanize")
                .feature_with("i18n", "i18n")
                .note("readable times, sizes, ordinals and numbers"),
        )
        .absent(
            "humanize",
            "no readable values; a page writes a raw timestamp and a byte count",
        )
    }

    fn contribute(&self, project: &Project, s: &mut Scaffold) {
        let file = if project.has("i18n") {
            HUMANIZE_I18N.to_string()
        } else {
            HUMANIZE_PLAIN.replace("@LOCALE@", &module_ref("humanize", "locale"))
        };
        s.file("src/plugins/humanize/mod.rs", file);
        s.at(Slot::PluginsMod).line("pub mod humanize;");
        s.at(Slot::AgentsSections).block(AGENTS.to_string());
    }
}

/// `src/plugins/humanize/mod.rs` without `i18n`: one language, from the
/// module file.
const HUMANIZE_PLAIN: &str = r#"//! Readable values for a page: "3 minutes ago", "1.5 MB", "2nd", "1,234".
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! ```ignore
//! let h = crate::plugins::humanize::of(ctx);
//! let line = format!("{}, {}", h.ago(seconds), h.file_size(bytes));
//! ```
//!
//! The language is `locale` in `src/modules/humanize.rs`: `en`, `fr`, `de`
//! or `es`. Another tag gets English. `brocanth add i18n` makes it follow
//! each request's locale.

pub use brocanth_humanize::Humanize;

use crate::pages::Ctx;

/// The helpers in this application's language.
#[allow(dead_code)]
pub fn of(_ctx: &Ctx) -> Humanize {
    Humanize::for_tag(@LOCALE@)
}

#[cfg(test)]
mod tests {
    use brocanth::routing::{Params, Verb};

    use super::*;

    #[test]
    fn a_page_writes_times_sizes_and_counts_for_a_reader() {
        let h = Humanize::for_tag("en");
        assert_eq!(h.ago(180), "3 minutes ago");
        assert_eq!(h.ago(-7200), "in 2 hours");
        assert_eq!(h.file_size(1_500_000), "1.5 MB");
        assert_eq!(h.ordinal(2), "2nd");
        assert_eq!(h.number(1_234_567), "1,234,567");
    }

    #[test]
    fn the_module_file_names_a_language_with_phrases() {
        let h = of(&Ctx::new(Verb::Get, "/", Params::empty()));
        assert_ne!(h.ago(120), "", "every language says something");
    }
}
"#;

/// `src/plugins/humanize/mod.rs` with `i18n`: the request's locale.
const HUMANIZE_I18N: &str = r#"//! Readable values for a page: "3 minutes ago", "1.5 MB", "2nd", "1,234".
//!
//! Written once by `brocanth create`. This file is yours.
//!
//! ```ignore
//! let h = crate::plugins::humanize::of(ctx);
//! let line = format!("{}, {}", h.ago(seconds), h.file_size(bytes));
//! ```
//!
//! The language is the locale the `i18n` hook chose for the request.
//! English, French, German and Spanish have their own phrases; another
//! locale gets English phrases. Numbers are grouped by the locale's CLDR
//! formats, as the catalogs in `locales/` group them.

pub use brocanth_humanize::Humanize;

use crate::pages::Ctx;

/// The helpers in the language of this request.
#[allow(dead_code)]
pub fn of(ctx: &Ctx) -> Humanize {
    Humanize::with_formats(crate::plugins::i18n::locale(ctx).formats())
}

#[cfg(test)]
mod tests {
    use brocanth::routing::{Params, Verb};

    use crate::plugins::i18n::Locale;

    use super::*;

    #[test]
    fn a_page_writes_times_sizes_and_counts_in_the_locale() {
        let en = Humanize::with_formats(Locale::En.formats());
        assert_eq!(en.ago(180), "3 minutes ago");
        assert_eq!(en.file_size(1_500_000), "1.5 MB");
        assert_eq!(en.ordinal(2), "2nd");
        let fr = Humanize::with_formats(Locale::Fr.formats());
        assert_eq!(fr.ago(180), "il y a 3 minutes");
        assert_eq!(fr.ordinal(1), "1er");
        // The catalog and the helper group a number alike.
        assert_eq!(fr.number(1_234_567), Locale::Fr.formats().number(1_234_567));
    }

    #[test]
    fn a_ctx_with_no_request_gets_the_default_locale() {
        // `en` is the default catalog.
        let h = of(&Ctx::new(Verb::Get, "/", Params::empty()));
        assert_eq!(h.ago(120), "2 minutes ago");
    }
}
"#;

/// What the generated `AGENTS.md` says about readable values.
const AGENTS: &str = "\n\
    ## Readable values\n\
    \n\
    A page writes a time, a size, an ordinal or a large count for a reader\n\
    through `crate::plugins::humanize::of(ctx)`: `ago(seconds)`,\n\
    `since(then, now)`, `file_size(bytes)`, `ordinal(n)`, `number(n)`. Do not\n\
    format these by hand: the helper follows the request's language, and\n\
    sizes are decimal (1 kB = 1000 bytes) everywhere, so two pages never\n\
    show one file in two units. Keep the exact value in a `title` or a\n\
    `<time datetime>` attribute beside the phrase.\n";
