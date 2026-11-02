# Docs

Every page in `docs/`, one line each. Each crate also has its own
`crates/<name>/README.md`; the root [`README.md`](../README.md) has the crate
map.

## Start here

| Page | What it covers |
|---|---|
| [getting-started.md](getting-started.md) | Install the CLI, create a project, run it, add pages, templates and commands. Real output. |
| [llm-authoring.md](llm-authoring.md) | The contract for writing brocanth with a model: what cannot compile, what needs a test. |
| [framework-gaps.md](framework-gaps.md) | The ranked gaps (G1 to G14) against Astro, Next.js, Django, Rails and PocketBase, with a status table. |

## Modules

| Page | What it covers |
|---|---|
| [modules/README.md](modules/README.md) | One page per module key: manifest, what it writes, its settings. |
| [module-map.md](module-map.md) | The 38 modules in five categories, with the dependency graph. |
| [modules.md](modules.md) | Where a crate attaches to a generated project: `Slot`s, the `scaffold` feature, the CLI registry. |
| [module-authoring.md](module-authoring.md) | How to write a module from zero: the contract, a walkthrough, every hook. |
| [module-flags.md](module-flags.md) | Declared settings: the config key, the flag, the environment variable, renames in `src/flags.rs`. |

## Runtime

| Page | What it covers |
|---|---|
| [async-handlers.md](async-handlers.md) | How a route gets to `.await`, and what it costs. A proposal with a prototype. |
| [input.md](input.md) | The entrance: how a request body becomes a struct, and the answer when it cannot. |
| [middleware.md](middleware.md) | The Gorilla toolkit against brocanth's middleware hooks. |
| [surface.md](surface.md) | One route table read by the OpenAPI document, the client and the handlers. |
| [policy.md](policy.md) | Predicates, `condition()`, `Verified`, and the `policy!` macro. |
| [observability.md](observability.md) | Logs, probes, metrics and traces in `brocanth-observe`. |
| [i18n.md](i18n.md) | Message catalogs checked at build time, locales, CLDR plurals. |
| [static-export.md](static-export.md) | `brocanth-static`: the pages compiled to files for a static host. |

## Admin

| Page | What it covers |
|---|---|
| [backoffice.md](backoffice.md) | What Django's admin and PocketBase's dashboard contain, and which module here owns each part. |
| [admin-customization.md](admin-customization.md) | Seven levels of change, from a title to the whole layout, against Django's way. |

## Data

| Page | What it covers |
|---|---|
| [migrations.md](migrations.md) | `brocanth migrate make`, stable field ids, and why a rename stays a rename. |
| [cache.md](cache.md) | Tags, the route cache, fragments and invalidation. |
| [auth-store.md](auth-store.md) | Where accounts live: `MemoryStore`, `PgStore`, the conformance suite. |

## Operations

| Page | What it covers |
|---|---|
| [tooling.md](tooling.md) | Fixtures, `dbshell`, `check --deploy`, `dp-prefetch`, and how to keep `target/` small. |

## Design records

| Page | What it covers |
|---|---|
| [../PLAN.md](../PLAN.md) | The design. §8 is modularity, §12 the build order, §13 the ADRs. |
| [../crates/brocanth-queue/docs/DECISIONS.md](../crates/brocanth-queue/docs/DECISIONS.md) | The queue design, its defects and its open items. |
| [pocketbase-surface.md](pocketbase-surface.md) | PocketBase v0.40.5's surface: what brocanth measures itself against, and what it declines. |
| [audit/README.md](audit/README.md) | The double-entry table: four frameworks, every capability, and whether brocanth has it. |
| [audit/django.md](audit/django.md) | Django 6.1 against brocanth. |
| [audit/rails.md](audit/rails.md) | Rails 8.1.4 against brocanth. |
| [audit/nextjs.md](audit/nextjs.md) | Next.js 16.4 against brocanth. |
| [audit/pocketbase.md](audit/pocketbase.md) | PocketBase v0.40.6 against brocanth. |
