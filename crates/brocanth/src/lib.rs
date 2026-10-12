//! Brocanth: a server framework for HTML-first applications.
//!
//! The Crystal original is a 1,272-line prototype, so this is a build rather
//! than a port. What carries over is its design, which was argued and
//! recorded: file-based routes with compile-time helpers, layouts nesting by
//! directory, pattern-scoped middleware, plugins that enrich config and
//! context explicitly rather than by autowiring, and a handler that returns a
//! value the framework turns into a response.
//!
//! What does not carry over is everything the original's shape forced. It
//! served GET only, had no persistence, and kept its registries in global
//! mutable state so two applications in one process collided. Each of those is
//! a Crystal-shaped workaround, and in Rust the type system deletes rather
//! than documents them.

pub mod config;
pub mod routing;
