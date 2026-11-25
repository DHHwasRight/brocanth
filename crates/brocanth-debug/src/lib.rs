#![cfg_attr(feature = "runtime", doc = include_str!("../README.md"))]
//!
//! # Decisions
//!
//! - **Inert in a release build, by the compiler.** [`ACTIVE`] is
//!   `cfg!(debug_assertions)`. False, the plugin installs no hook, so no
//!   response is ever rewritten, whatever the settings say. A release build
//!   that keeps the module by mistake shows nothing; `check --deploy`
//!   catches the mistake before that.
//! - **A response hook, not a page.** The panel goes into every HTML answer
//!   (a 404 too) before `</body>`, by the rule the analytics beacon and the
//!   consent banner follow. A fragment (`dp-partial`) gets nothing.
//! - **SQL from the ORM's probe.** SeaORM reports each statement to a
//!   callback, and [`brocanth_model::Probe`] groups them by shape. The probe
//!   sees the whole process: two requests at once each count the other's
//!   statements. That is fine for one developer and wrong for a load test,
//!   which is one more reason the module never ships.
//! - **The hooks are the chain, not a trace.** The panel lists the hooks of
//!   each chain in run order. A request hook that answers stops the ones
//!   after it; the panel says which chain ran, not which hook answered.

#[cfg(feature = "scaffold")]
pub mod scaffold;

#[cfg(feature = "runtime")]
mod toolbar;

#[cfg(feature = "runtime")]
pub use toolbar::{DebugToolbar, ToolbarConfig, ACTIVE};
