# Next.js vs brocanth

**Next.js version checked: 16.4, released 2026-10-06.** Verified on 2026-10-26
against `nextjs.org/blog` and `nextjs.org/docs/app/api-reference/config/next-config-js`.
16.4 is still the newest release; everything after it on the blog is a security
patch. N1–N43 come from `docs/framework-gaps.md` §1.2 and keep their numbers.
N44–N51 are capabilities that inventory missed; they are new here.

**Every brocanth verdict was read out of the tree on 2026-10-26.**
`docs/framework-gaps.md` §2.2 was not used as a source. Where §2.2 and this
table disagree, this table is the later reading.

**Two standing constraints decide every `n/a` row:** brocanth has no
client-side component runtime, and no React. A capability is `n/a (out of
scope)` only when it cannot exist without a hydrating client tree. A
capability that Next happens to deliver through React, but that a server can
deliver on its own, is judged on whether brocanth delivers it.

## The matrix

| ID | Capability | Next | brocanth | Evidence |
|---|---|---|---|---|
| N1 | Nested layouts per segment, preserved across navigation; root layout renders `<html>`/`<body>` | yes | partial | `crates/brocanth-pages/src/lib.rs` — `html!` walks up from the page's own file and stacks every `layout.rsx` it passes; the root one emits `<html>`/`<body>` in `crates/brocanth-cli/src/modules/rsx.rs` (`LAYOUT_RSX`). "Preserved across navigation" is approximated server-side: `create.rs` `HOLDING_FN` reads the `DP-Partial` header and `html!` omits that many layers, and dp-swap replaces `<main dp-layout>`. No DOM identity is preserved — the shared sub-tree is re-sent as markup, not kept. |
| N2 | `template.tsx`: a new instance per navigation, effects re-run | yes | n/a (out of scope) | Client remount semantics. Needs a hydrating client tree; brocanth has no client-side component runtime. |
| N3 | `page.tsx` makes a segment routable; `params` / `searchParams` | yes | yes | `crates/brocanth/src/routing_path.rs` — `index.rs` → `/`, `[id].rs` → `/users/{id}`, `[...rest].rs` → `/docs/{*rest}` (tests at lines 611–706). `crates/brocanth-pages/src/table.rs` emits the table. `Ctx` carries `path`, `query`, `params` and `headers` (`crates/brocanth-cli/src/commands/create.rs:2242`). |
| N4 | `loading.tsx` auto-wraps the segment in `<Suspense>` with itself as the fallback | yes | no | A generated project cannot stream. `create.rs:2463` (`SENT_FN`) builds `brocanth_server::Response::html(status, body)` from a `Response` whose `body` field is a `String`. `Body::Streaming` exists in `crates/brocanth/src/plugin_body.rs` and `brocanth_server::Response::stream` exists (`crates/brocanth-server/src/lib.rs:248`), but no path reaches a page handler. |
| N5 | `error.tsx`: per-segment error boundary, `'use client'`, `retry()` | yes | n/a (out of scope) | `retry()` re-requests one segment into a live tree. Needs a client component runtime. The server half of the idea does exist: `create.rs:2215` withholds the message of any status ≥ 500, which is `error.digest` redaction. |
| N6 | `global-error.tsx` replaces the root layout and renders its own `<html>` | yes | n/a (out of scope) | Document-level client boundary. No client runtime. |
| N7 | `not-found.tsx` with `notFound()`; `forbidden()` / `unauthorized()` | yes | partial | A handler can return any status with a rendered body — `impl IntoResponse for (T, u16)` at `create.rs:2130`, and `Error` carries a status. But there is no `not-found` file convention anywhere in `crates/brocanth-pages/`, and the router's own miss is plain text: `create.rs:1818`, `Dispatch::NotFound => Response::new(404, "Not Found")`. Nothing throws-to-render. |
| N8 | `default.tsx` for an unmatched parallel slot; required since 16.0 | yes | n/a (out of scope) | Downstream of N9. |
| N9 | Parallel routes `@slot` as named layout props, each streaming independently | yes | n/a (out of scope) | The defining part is per-slot navigation state in a client router. Without one, a slot is an ordinary component the layout composes, which rsx already does. |
| N10 | Intercepting routes `(.)` / `(..)` / `(...)` | yes | n/a (out of scope) | Needs a client router that tells a soft navigation from a hard one, plus History API URL masking. No client runtime. |
| N11 | Route groups `(name)`: organise folders with no URL segment | yes | no | `crates/brocanth/src/routing_path.rs` has three file rules and none of them is a group; the route tests (lines 611–739) show `[id]` and `[...rest]` only. Nothing in the tree. Build-time only, so nothing blocks it. |
| N12 | `route.ts` handlers over `Request` / `Response`; generated `RouteContext` types | yes | yes | `crates/brocanth-pages/src/lib.rs` — the verb comes off the handler's name (`handle` = GET, `handle_post` = POST), parsed out of the page's own source; a `pub fn handle_*` that is not a verb is a build error. `Response::json` at `crates/brocanth-server/src/lib.rs`, `Json` in the generated `src/pages/mod.rs` (`create.rs:4350`). The typed-route half is better than Next's: `routes::users::id::path(id)` (`create.rs:2836`) is a compiler-checked function, not a string literal type. |
| N13 | `proxy.ts`: one root file, matcher config, rewrite / redirect / headers / respond | yes | yes | `crates/brocanth/src/plugin_hooks.rs:390` — `RequestHook::before_dispatch` returning `Flow::Respond` short-circuits the handler; `crates/brocanth/src/routing_middleware.rs` is the pattern-scoped (matcher) form. Five are shipped as modules: `crates/brocanth-middleware/src/` — `ProxyHeaders`, `MethodOverride`, `CanonicalHost`, `ContentTypes`, `Cors`. It is a plugin rather than one root file, which is a shape difference, not a capability gap. |
| N14 | Server Components rendering to an RSC Payload | yes | n/a (out of scope) | No React, no client runtime, no wire format to reconcile against. The nearest thing is a depth integer: `DP-Partial` plus `html!`'s layer skipping (`create.rs` `HOLDING_FN`). |
| N15 | `"use client"` as a module-graph boundary; `server-only` / `client-only` | yes | n/a (out of scope) | There is no client module graph to split. |
| N16 | `"use server"` / Server Functions, POST only, each doing its own authz | yes | partial | The server half is there and reachable without a client: `handle_post` routes (`create.rs:2833`), `crates/brocanth-input/` turns the body into a struct, `crates/brocanth-schema/` validates it into a `Valid<T>`, `crates/brocanth-policy/` is the rule algebra for the authz. What is missing is the binding — a typed function a form can name — and the client verb: `crates/brocanth-cli/vendor/dpswap.min.js` has `dp-get` and no `dp-post`. |
| N17 | Server Actions in forms: `<form action>` with `FormData`, submits with JS off | yes | yes | The no-JS path is the only path brocanth has, and it is complete: a form POSTs to the page's `handle_post`, `brocanth-input` decodes it, and the handler answers `Response::empty(303).with_header("location", routes::users::id::path(id))` (`create.rs:2836`). The single-roundtrip enhancement needs the client runtime and is out of scope. |
| N18 | `useActionState(action, initialState)` → `[state, formAction, pending]` | yes | n/a (out of scope) | Client React runtime over the POST. |
| N19 | Revalidation from an action: `updateTag`, `refresh`, `revalidatePath`, `revalidateTag`, `redirect` | yes | partial | `redirect` works (`create.rs:2836`). Tag invalidation does not: `crates/brocanth-cache/src/` has `Namespace::versioned("views", "v1")` and a key, and the only word for "invalidate everything of this kind" is bumping the version string. See N22. |
| N20 | `'use cache'` keyed by build ID + location + signature + serialized args + captured scope | yes | no | `crates/brocanth-cache/src/fetch.rs` is read-through `fetch(&key, ttl, closure)` with a key the caller writes by hand. There is no compiler pass deriving a key from closure capture anywhere in the tree. |
| N21 | `cacheLife(profile)` with three windows: `stale`, `revalidate`, `expire` | yes | no | `crates/brocanth-cache/src/lib.rs` — one `Duration` per `fetch`. No profiles and no three-tier TTL. |
| N22 | `cacheTag(tag)` / `revalidateTag(tag, profile)` | yes | no | `grep -i tag crates/brocanth-cache/src/` returns one hit and it is prose about staggering TTLs (`lib.rs:84`). No tag index. |
| N23 | Three cache stores: prerendered output, a pluggable shared server store, the browser | yes | partial | One store, and it is pluggable: `Cache` is a trait, `PgCache` is the implementation (`crates/brocanth-cache/src/postgres.rs`), the table is `UNLOGGED` on purpose. There is no prerendered-output store (nothing prerenders) and no browser payload cache (no payload). |
| N24 | Partial Prerendering: a static shell with request-time holes streamed into it | yes | no | Needs N4's streaming, which a generated project does not have (`create.rs` `SENT_FN`), and a prerender step, which does not exist. |
| N25 | ISR and the App Shell: `generateStaticParams`, background regeneration, write-back | yes | no | Nothing in the tree prerenders a URL or regenerates one. Every page in `crates/brocanth-pages/src/table.rs` is dispatched per request. |
| N26 | `ensureStatic` (16.4): fails the build if dynamic content can reach a route | yes | no | Nothing in the tree. Presupposes N24/N25. |
| N27 | Streaming as an enforced contract: an uncached async read must sit behind `<Suspense>` or `use cache` | yes | no | Nothing in the tree, and the contract has nothing to enforce while N4 is absent. |
| N28 | `navigation()` / `prefetch()` from `next/cache` (16.4): exclude a subtree from a prefetch or a shell | yes | n/a (out of scope) | These name the stages of producing an RSC payload for a client payload cache. Without a client component runtime there are no three stages to exclude from. |
| N29 | `next/link` prefetching: viewport entry, hover re-fire, cancel on exit, layout dedup. GET only | yes | no | `crates/brocanth-cli/vendor/dpswap.min.js` exposes `dp-boost`, `dp-get`, `dp-layout`, `dp-target`, `dp-swap-oob`, `dp-island`, `dp-auto`, `dp-request`, `dp-settling` — and no prefetch verb. `crates/brocanth-cli/src/modules/rsx.rs` writes `src/pages/link.rsx`, which is an anchor and nothing more. This needs no mutating verb and no React. |
| N30 | `next/form`: `<Form action="/search">` submits as a GET that updates search params | yes | partial | A plain `<form method="get">` works and the query string reaches the handler (`Ctx.query`, `create.rs:2250`), so the baseline is there. The framework ships no `<Form>` component, no search-param helper and no prefetch of the destination. `crates/brocanth-cli/src/modules/rsx.rs` writes `link.rsx` only. |
| N31 | `generateMetadata` and the static `metadata` export; streaming metadata | yes | no | `crates/brocanth-cli/src/modules/rsx.rs` (`LAYOUT_RSX`) hardcodes `<title>{TITLE}</title>` from one `const` in `src/pages/mod.rs`, and `crates/brocanth-pages/src/lib.rs` states the limit outright: `html!` fills `children`, optionally `layout`, and **no other parameter**. A page therefore has no way to give its own layout a title, a description or an OG tag. Fully reachable without a client. |
| N32 | Bot detection: `htmlLimitedBots`, a UA-keyed branch between rendering strategies | yes | no | Nothing in the tree reads `user-agent` for a rendering decision. Presupposes N24 and N31, neither of which exists. |
| N33 | File-based metadata: `opengraph-image.*`, `icon.*`, `robots.txt`, `sitemap.xml`, plus generators | yes | partial | Static half works: `crates/brocanth-assets/src/lib.rs` serves `public/` verbatim, named for `robots.txt` and `favicon.ico` in its own header, and `create.rs` `STATIC_FN` adds an ETag, a 304 and a `cache-control`. Dynamic half half-works by accident: `crates/brocanth/src/routing_path.rs:701` proves `src/pages/sitemap.xml.rs` routes to `/sitemap.xml`, so a generated sitemap is an ordinary page. There is no `opengraph-image` convention, no `icon` convention and no `generateSitemaps`. |
| N34 | `ImageResponse` from `next/og`: JSX and CSS to PNG | yes | no | Nothing in the tree. This is a server-side layout and raster engine — no client runtime involved. |
| N35 | `next/image`: `srcset` against an optimiser endpoint, `Accept` negotiation, `placeholder="blur"`, `remotePatterns` | yes | no | `crates/brocanth-assets/src/lib.rs` is byte-for-byte serving with a content hash; its only image awareness is the MIME map at lines 291–300. No resize, no re-encode, no negotiation, no endpoint. |
| N36 | `next/font`: Google fonts downloaded at build and self-hosted; `adjustFontFallback` | yes | partial | Self-hosting and cache-forever naming are solved — `crates/brocanth-assets/src/lib.rs` maps `woff2`/`woff`/`ttf` (lines 298–300) and fingerprints anything in `assets/`. Nothing fetches a font at build time, subsets it, or extracts metrics for a zero-CLS fallback. |
| N37 | i18n routing. Next ships **none**; the documented path is `proxy.ts` + `app/[lang]/` + `next/root-params` | no built-in | partial | Both primitives Next's recipe uses are present: `[lang]` is an ordinary dynamic segment (`crates/brocanth/src/routing_path.rs`) and a request hook can redirect or rewrite on `accept-language` (`crates/brocanth-middleware/src/`, `crates/brocanth/src/plugin_hooks.rs:390`); `Ctx` carries the plugin context so a handler reads the locale without prop drilling (`create.rs:2259`). What is absent is everything past routing: `grep -i 'i18n\|locale\|gettext' crates/` returns nothing in brocanth's own source. Neither framework ships a message catalogue. |
| N38 | Turbopack by default for dev and build; disk cache, lazy server HMR, shared runtime chunk | yes | partial | Incremental builds are cargo's, and templates, routes, layouts and assets all compile into the binary (`rsx::load_dir!`, `crates/brocanth-pages/src/lib.rs`, `crates/brocanth-assets/src/lib.rs`), so there is nothing to bundle. There is no dev server and no hot reload: the only watcher in the tree is the Tailwind one, `crates/brocanth-cli/src/modules/rsx.rs:680` (`css:watch`). |
| N39 | `instrumentation.ts`: `register()` once per server instance; `onRequestError` with `routeType` and `renderType` | yes | partial | The once-per-instance hook exists: `Plugin::configure` and `Plugin::start` (`crates/brocanth/src/plugin_api.rs:381,417`). The error hook exists: `ErrorHook::on_error` (`crates/brocanth/src/plugin_hooks.rs:677`), and `RequestHook::after_dispatch` sees every outcome including a short-circuit or a hook failure. No OpenTelemetry wiring is shipped, and there is no rendering-phase context to report because there are no rendering phases. |
| N40 | Draft Mode: `__prerender_bypass` re-executes every `'use cache'` scope without writing back | yes | no | `grep -i 'draft_mode\|prerender_bypass' crates/` returns nothing in brocanth's source. `crates/brocanth-cache/` has no per-request bypass flag of any kind. Reachable without a client: a signed cookie plus a flag on `RequestState`. |
| N41 | `after()` from `next/server`: schedule work for after the response has streamed | yes | partial | `RequestHook::after_dispatch` runs after the response is final (`crates/brocanth/src/plugin_hooks.rs:401`), but before the transport writes, and its own doc says so: *"Waiting there holds the connection open to do work the client is not waiting for; a plugin that wants that wants a job queue."* That queue exists and is good — `crates/brocanth-jobs/` with retries, backoff and a dead letter — but it is durable enqueue with a worker pool, not fire-and-forget after the last byte. |
| N42 | `next typegen` without a full build; `next analyze` with import-chain tracing | yes | yes | Better by construction. The route table, the layout stack and the route helpers are compiled, so a wrong route is a compile error with nothing to regenerate: `crates/brocanth-pages/src/table.rs`, `routes::users::id::path` (`create.rs:2836`). `pages!("src/pages", describe)` emits what every route takes and gives (`crates/brocanth-pages/src/lib.rs`), and `crates/brocanth-openapi/src/lib.rs` `check_file` fails the build when the committed document drifts. There is no bundle analyzer because there is no bundle. |
| N43 | `next upgrade --agent`, `experimental.agentUpgrade` / `agentFeedback`, a DevTools MCP server, published Skills | yes | partial | The scaffold writes an `AGENTS.md` and modules extend it through a slot: `create.rs:874,879` with `Slot::AgentsSections`; `crates/brocanth-cli/src/owned.rs:63` marks it owned by the author so a regeneration does not stamp over their notes. `docs/llm-authoring.md` is the hand-written half. No upgrade agent, no MCP server, no skills. |
| N44 | Adapter API (stable in 16.2) and a public adapter test suite: one build, many deployment targets | yes | no | Nothing in the tree. `brocanth_server::Server::bind` (`crates/brocanth-server/src/lib.rs`) is the single entry point and the output is one native binary, which makes most of what an adapter does unnecessary rather than missing. |
| N45 | `next.config` `redirects()`, `rewrites()`, `headers()`, plus `basePath`, `assetPrefix`, `trailingSlash` | yes | partial | Every one of these is achievable as a hook, and two ship: `crates/brocanth-middleware/src/canonical_host.rs` redirects and `crates/brocanth-middleware/src/cors.rs` sets headers. What is missing is the declarative form — `crates/brocanth-config/src/lib.rs` has layered TOML with provenance and an error for an unclaimed section, but no module claims a `[redirects]` or `[headers]` section. |
| N46 | `public/` served at the root URL; build assets content-hashed and cached forever | yes | yes | `crates/brocanth-assets/src/lib.rs` — `assets!("assets", "public")`, `assets/` fingerprinted to `css/site.a1b2c3d4.css` and `public/` verbatim; `assets::url()` is a compile error on a name that does not exist, which Next's `/public` is not. `create.rs` `STATIC_FN` adds the strong ETag, the 304 and the `cache-control`, and checks files before the router. |
| N47 | Built-in CSS pipeline: global CSS, CSS Modules, Sass, PostCSS; Tailwind in `create-next-app` | yes | partial | Tailwind only, and it needs npm: `crates/brocanth-cli/src/modules/rsx.rs` (`CSS_RS`) writes a project command that shells out to `node_modules/.bin/tailwindcss` with no fallback (line 592), plus a `css:watch` mise task. No CSS Modules, no Sass, no PostCSS config. Assets are hashed by N46 either way. |
| N48 | `output: 'standalone'` self-host build, and `next start` | yes | yes | This is brocanth's default and its premise. One `cargo build` yields a binary carrying the templates (`rsx::load_dir!`), the route table (`crates/brocanth-pages/src/table.rs`), the layouts and the static files (`crates/brocanth-assets/src/lib.rs`, embedded with `include_bytes!` rather than read from disk). Even dp-swap is vendored so a scaffold works offline (`crates/brocanth-cli/vendor/README.md`). |
| N49 | Async request APIs: `cookies()`, `headers()`, `connection()` — read request state anywhere in the tree without prop drilling | yes | partial | A handler has all of it: `Ctx` carries `verb`, `path`, `query`, `body`, `params`, `headers` and the plugin context (`create.rs:2242–2259`), and sessions are a module (`crates/brocanth-cli/src/modules/sessions.rs`). A **layout** has none of it: `crates/brocanth-pages/src/lib.rs` states that `html!` fills `children` and optionally `layout` and *"no other parameter"*. So a layout cannot read a cookie, a header or the current user. |
| N50 | MDX / Markdown compiled into the route tree (`@next/mdx`, `mdxRs`) | yes | no | Nothing in the tree. `rsx::load_dir!` compiles `.rsx` only. The seam is proven, though: `Slot::PagesRenderers` is how `crates/brocanth-cli/src/modules/jt.rs` attached a second engine over the same page tree with a manifest, a dependency and one block. |
| N51 | `create-next-app` scaffolding, and `next dev` with an error overlay | yes | partial | Scaffolding is ahead: `crates/brocanth-cli/src/modules/mod.rs` `ALL` is a written menu of 22 modules, each with a gist, an `after` list, a `needs` list with a reason, and a `blocked` reason printed on its own row; `brocanth add` adds one later, and unknown subcommands are delegated to the project binary (`crates/brocanth-cli/src/main.rs:190`). The dev loop is behind: `brocanth` has exactly two subcommands of its own, `create` and `add` — no `dev`, no `build`, no `routes`, no error overlay. |

## Counts

| Verdict | Rows |
|---|---|
| yes | 7 |
| partial | 17 |
| no | 17 |
| n/a (out of scope) | 10 |
| **total** | **51** |

The ten `n/a` rows are N2, N5, N6, N8, N9, N10, N14, N15, N18 and N28. Every
one of them needs a hydrating client tree, and nine of the ten need React's
own semantics on top of that. No row was marked `n/a` because Next uses React
to build something a server could build alone.

## Worth building, ranked

Ten gaps that are high value for brocanth **and** need no client-side
component runtime. Sizes assume one person who already knows the tree.

1. **Let a page handler return a streaming body.** (N4, N24, N27, and SSE.)
   The core already streams — `Body::Streaming` in
   `crates/brocanth/src/plugin_body.rs`, `Response::stream` and
   `Response::events` in `crates/brocanth-server/src/lib.rs` — and the only
   thing in the way is that a generated project's `Response.body` is a
   `String` (`create.rs` `SENT_FN`). One type change unblocks the streamed
   shell, the SSE module that is currently `blocked("the core can stream now;
   this transport is simply not written yet")`, and every large download.
   **Size: 2–3 days**, mostly in `create.rs` and the `IntoResponse` impls.

2. **Per-page metadata.** (N31.) Today every page in a project shares one
   `const TITLE`, because `html!` fills `children` and nothing else. A site
   with one `<title>` is not shippable, so this is the gap every real user
   hits on day one. **Size: 2 days** — give `html!` a metadata value the page
   returns alongside its element, and let a layout take it as a parameter.

3. **Cache tags and tag invalidation.** (N22, and half of N19.) The cache is
   good and the invalidation story is "bump a version string"
   (`Namespace::versioned`). Tags are what turn `brocanth-cache` from a
   key/value store into something a page cache can be built on, and they are
   a table and an index in a database that is already there.
   **Size: 3–4 days.**

4. **Prefetch in dp-swap.** (N29.) Next's single largest navigation
   subsystem is pure GET: viewport entry, hover re-fire, cancel on exit. It
   needs no mutating verb, so it is the one big win that is not blocked on
   the `dp-post` work, and dp-swap already knows how to swap by layout depth.
   **Size: 3 days** in the dp-swap checkout, plus a cache-control decision
   here.

5. **`not-found.rsx` and an error page convention.** (N7.) A generated app
   answers a miss with the bytes `Not Found`. Giving the page tree the same
   file rule it already has for layouts — a `not-found.rsx` covers its
   directory and everything below — reuses `html!`'s walk and costs almost
   nothing. **Size: 1 day.**

6. **`after()`: work that runs once the bytes are gone.** (N41.)
   `after_dispatch` is explicitly documented as the wrong place to wait, and
   `brocanth-jobs` is heavier than an analytics write needs. A small
   post-response queue the transport drains closes the gap between the two.
   **Size: 1–2 days**, in `brocanth-server`'s bridge.

7. **Route groups `(name)`.** (N11.) A fourth rule in
   `crates/brocanth/src/routing_path.rs`, a fourth row in its table, and a
   handful of tests. It is the cheapest row in this list and it is what lets
   a project give a section its own root layout without inventing a URL
   segment. **Size: 4 hours.**

8. **An image endpoint.** (N35.) `brocanth-assets` already owns the bytes,
   the content hash and the `cache-control`, so resize plus `Accept`-driven
   re-encode plus a `srcset` helper is an addition to a crate that is already
   the right home. Pure server work. **Size: 4–5 days**, most of it choosing
   and sandboxing a decoder.

9. **i18n routing.** (N37.) Next ships nothing here, so this is a place to
   be ahead rather than catch up, and both primitives exist already — a
   `[lang]` segment and a hook that can redirect on `accept-language`. The
   work is the catalogue and the lookup, which can be a compiled module like
   everything else. **Size: 4–5 days.**

10. **Markdown pages through `Slot::PagesRenderers`.** (N50.) Blogs and docs
    are what a server-rendered framework with no client runtime is best at,
    and `jt` already proved a second engine attaches with a manifest, a
    dependency and one block. A `.md` page that lands in the same route table
    as a `.rsx` one is a module, not a core change. **Size: 3 days.**

**Deliberately below the line.** OG image generation (N34) is high value and
a week of raster work; draft mode (N40) is a day but needs N22's tags to be
worth having; font fetching and subsetting (N36) is a build step that only
pays off after N31 gives a page a `<head>` it controls.
