//! Every module this CLI was compiled with.
//!
//! A module is a capability `brocanth create` can offer. The contract is
//! [`brocanth_scaffold::Module`], and what matters here is that [`ALL`] is a
//! written list: one line per module, and a line a reader can delete.
//!
//! # Adding somebody else's module
//!
//! Two lines, and then rebuild the CLI:
//!
//! ```toml
//! # crates/brocanth-cli/Cargo.toml
//! brocanth-module-stripe = { git = "https://github.com/you/brocanth-module-stripe" }
//! ```
//!
//! ```ignore
//! // here, in ALL
//! &brocanth_module_stripe::Stripe,
//! ```
//!
//! ```text
//! cargo install --path crates/brocanth-cli
//! ```
//!
//! There is no distributed slice and no plugin directory scanned at run time.
//! `brocanth_scaffold`'s own documentation gives the argument; the short form
//! is that linking follows the transitive graph, so a module that registered
//! itself could arrive through a dependency nobody read, and this menu is the
//! receipt for what a generated binary contains.
//!
//! # Where a module lives
//!
//! Beside the crate it scaffolds, behind a `scaffold` feature that is off by
//! default, so an application that depends on `brocanth-cache` never links
//! the code that writes `brocanth-cache` into a project. The modules kept
//! here are the ones with no crate of their own: a logger the scaffold
//! writes out in full, cookie sessions, and the three faces of rsx.

use brocanth_scaffold::Module;

pub mod commands;
pub mod config;
pub mod logging;
pub mod model;
pub mod rsx;
pub mod sessions;

/// The modules, in the order the chooser lists them and the receipt prints
/// them.
///
/// Order here is presentation only. Which module *contributes* first is
/// decided by the `after` lists in the manifests, so two modules writing into
/// one slot never depend on this array.
pub const ALL: &[&dyn Module] = &[
    &logging::Logging,
    &config::Config,
    &sessions::Sessions,
    &model::Model,
    &commands::Commands,
    &rsx::Rsx,
    &rsx::Tailwind,
    &rsx::Htmx,
];

/// The module with this key, if this CLI has one.
pub fn find(key: &str) -> Option<&'static dyn Module> {
    ALL.iter().copied().find(|m| m.manifest().key == key)
}

/// Every key, in listing order.
pub fn keys() -> Vec<&'static str> {
    ALL.iter().map(|m| m.manifest().key).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_is_spelled_once() {
        let mut keys = keys();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "two modules share a key");
    }

    #[test]
    fn a_parent_and_an_after_name_a_module_that_exists() {
        // The one way a module can be wrong that the compiler cannot see: a
        // key is a string, so a renamed module leaves a dangling reference
        // in somebody else's manifest.
        for module in ALL {
            let m = module.manifest();
            if let Some(parent) = m.parent {
                assert!(
                    find(parent).is_some(),
                    "`{}` is part of `{parent}`, which no module declares",
                    m.key
                );
            }
            for need in &m.after {
                assert!(
                    find(need).is_some(),
                    "`{}` waits for `{need}`, which no module declares",
                    m.key
                );
            }
        }
    }

    #[test]
    fn a_child_is_named_after_the_choice_it_is_part_of() {
        // The command line is flat, so the name carries what the chooser
        // carries with an indent.
        for module in ALL {
            let m = module.manifest();
            match m.parent {
                Some(parent) => {
                    assert!(
                        m.key.starts_with(&format!("{parent}-")),
                        "`{}` is part of `{parent}` and should say so",
                        m.key
                    );
                    assert!(!m.because.is_empty(), "`{}` must say why", m.key);
                }
                None => assert!(!m.key.contains('-'), "`{}` is part of nothing", m.key),
            }
        }
    }

    #[test]
    fn every_module_says_what_it_costs() {
        for module in ALL {
            let m = module.manifest();
            assert!(!m.gist.is_empty(), "`{}` has no gist", m.key);
            // It is set beside seven others in a column, so it is a phrase
            // and not a sentence. A full stop there reads as a typo.
            assert!(
                !m.gist.ends_with('.'),
                "`{}`'s gist is a column entry, not a sentence: {:?}",
                m.key,
                m.gist
            );
        }
    }
}
