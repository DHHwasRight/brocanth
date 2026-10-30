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
//! # A module that is listed and not available
//!
//! `Manifest::blocked` puts a module on the menu with the reason it cannot
//! be chosen on its own row, and the cursor steps over it. That is not the
//! same as leaving it out. The generated manifest already says what a
//! project does not contain and why; a chooser that silently omits the two
//! transports realtime has no core support for answers "where is SSE" with
//! nothing, and somebody asks again next month.
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

pub mod ci;
pub mod commands;
pub mod config;
pub mod docker;
pub mod jt;
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
    // Observability, from the crate that serves it: a parent and four
    // children, each a cargo feature or a pair of paths. Its logs child
    // replaces `logging`: `create` refuses the two together.
    &brocanth_observe::scaffold::Observe,
    &brocanth_observe::scaffold::Logs,
    &brocanth_observe::scaffold::Health,
    &brocanth_observe::scaffold::Metrics,
    &brocanth_observe::scaffold::Tracing,
    &config::Config,
    &sessions::Sessions,
    &model::Model,
    // From `brocanth-cache` itself, which is the point: a module can live in
    // the crate it scaffolds, and joining the menu is still one line here.
    &brocanth_cache::scaffold::Cache,
    // The queue, from the crate that is the queue.
    &brocanth_jobs::scaffold::Jobs,
    // Accounts, from the crate that is the accounts. The local path only:
    // there is one `AuthStore` in the workspace and it is a `HashMap`, and
    // a provider cannot be a second choice until a crate can appear in two
    // modules' dependency lists.
    &brocanth_auth::scaffold::Auth,
    // The rule algebra. `after("model")` and not `needs`: a condition
    // inverts into a broadcast key as well as a `WHERE`, so requiring an
    // ORM would refuse a project that broadcasts and stores nothing.
    &brocanth_policy::scaffold::Policy,
    // Uploads on a local disk. The first module to write a `.gitignore`
    // line, which is also why that slot is now read.
    &brocanth_storage::scaffold::FileStorage,
    // The five edge hooks. The first crate in the workspace whose whole
    // job is to implement `HookSource`: the pipeline was there, tested,
    // and every implementation of it was a test fixture.
    &brocanth_input::scaffold::Entry,
    &brocanth_client::scaffold::Client,
    &brocanth_middleware::scaffold::Middleware,
    // The sixth hook in that crate, and the only one that arrives ticked:
    // the other five are a fact about somebody's network, and this one is
    // a fact about browsers.
    &brocanth_middleware::scaffold::Csrf,
    // Policy headers ride with the middleware; rate limiting waits to be
    // asked for, because its rules are a fact about one application.
    &brocanth_middleware::scaffold::SecurityHeaders,
    &brocanth_middleware::scaffold::RateLimit,
    // The document, served at `/openapi.json` as a page, because a route
    // here is a file.
    &brocanth_openapi::scaffold::OpenApi,
    // The schema vocabulary. Keyed `validation`, because what a project
    // gets is a `Valid<T>` rather than a schema library.
    &brocanth_schema::scaffold::Validation,
    // Typed CRUD routes. It writes the page triple and an empty list of
    // resources mounted at `/api`, and no entity: a resource is one table,
    // and a new project has none to name.
    &brocanth_resource::scaffold::Records,
    // The backoffice, mounted at `/admin` through `Slot::RouterMounts` and
    // locked until the application says who may in. Its SeaORM source is
    // listed and not available: the generated model plugin opens no pool.
    &brocanth_admin::scaffold::Backoffice,
    &brocanth_admin::scaffold::BackofficeModels,
    // The broker, and no transport: see its own module documentation for
    // what the core must change before there is one.
    &brocanth_realtime::scaffold::Realtime,
    // Listed and not available, both of them. The reason is on the row:
    // the core's response body is finished before a hook sees it, so a
    // transport that streams is a change to the hook contract.
    &brocanth_realtime::scaffold::Sse,
    &brocanth_realtime::scaffold::WebSocket,
    &commands::Commands,
    &rsx::Rsx,
    // The second renderer, and the reason there is a seam for one. It
    // attaches with a manifest, a dependency and one block: nothing in
    // `brocanth-pages`, `create.rs` or the route table knows it exists.
    &jt::Jt,
    &rsx::Tailwind,
    &rsx::Htmx,
    // Shipping: the image, a deployment of it, and the checks on every
    // push, each a choice because each is one way among several.
    &docker::Docker,
    &docker::Kubernetes,
    &ci::Ci,
    &ci::GitHub,
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
    fn a_requirement_names_a_module_that_exists_and_says_what_breaks() {
        for module in ALL {
            let m = module.manifest();
            for (need, why) in &m.needs {
                assert!(
                    find(need).is_some(),
                    "`{}` needs `{need}`, which no module declares",
                    m.key
                );
                assert_ne!(*need, m.key, "`{}` needs itself", m.key);
                // It is printed to whoever named this choice alone, so it
                // has to say what breaks rather than restate the arrow.
                assert!(
                    !why.is_empty(),
                    "`{}` needs `{need}` and does not say why",
                    m.key
                );
            }
        }
    }

    #[test]
    fn a_module_nobody_can_choose_says_why_on_its_own_row() {
        for module in ALL {
            let m = module.manifest();
            let Some(why) = m.blocked else { continue };
            assert!(
                !why.is_empty(),
                "`{}` is blocked and does not say why",
                m.key
            );
            // It is read in a row beside a label, not in a paragraph.
            assert!(
                why.len() <= 80 && !why.contains('\n'),
                "`{}`'s reason has to fit on its row: {why:?}",
                m.key
            );
            assert!(
                !m.on_with_parent,
                "`{}` cannot be ticked, so it must not start ticked",
                m.key
            );
            assert!(
                m.absent.is_none(),
                "`{}` is not absent from a project, it is unavailable: the manifest \
                 must not list it as a thing that was not asked for",
                m.key
            );
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
