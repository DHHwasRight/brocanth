//! Every task in this project.
//!
//! The `mod` lines are the whole of it. A task registers itself through a
//! distributed slice, so this file never names one -- but the modules still
//! have to be compiled, and that is what these lines do.

pub mod cache_clear;
pub mod seed_demo;
