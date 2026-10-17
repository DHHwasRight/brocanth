//! Every static file this binary serves.
//!
//! Two directories, two rules. `assets/` is **fingerprinted** -- `site.css`
//! is published at `/css/site.<hash>.css`, so it can be cached forever and a
//! new build invalidates it by having a different name. `public/` is served
//! **verbatim**, because `robots.txt`, `favicon.ico` and anything linked to
//! from outside must keep the name they were given.
//!
//! Write a reference with `assets::url`, never by hand:
//!
//! ```ignore
//! <link rel="stylesheet" href={assets::url("css/site.css")}>
//! ```
//!
//! A name nothing publishes panics there rather than 404ing in a browser.
//!
//! Embedded, like the templates and the route table, so `serve` works from
//! any directory and nothing can go missing after the build passed. A file
//! added where there was none needs `touch src/assets.rs`, because nothing
//! referenced it yet.

brocanth_assets::assets!("assets", "public");
