# Framework gaps

What Astro, Next.js, Django, Rails and PocketBase provide, what brocanth has
today, and what sits between them.

Part 1 comes from current documentation, fetched 2026-10-19, with the version
named in each section. Part 2 comes from reading this checkout at
`framework-plan` (forked from `2c45fe6`). Where Part 2 says a thing is absent,
somebody read the code. Where it says a thing is present, somebody read the
code too — a crate in `Cargo.toml` is not evidence.

Nothing here was run. No test was executed and no project was generated. Every
claim about brocanth is a claim about source text.

Three mechanisms bound every proposal, and a gap that fights one of them is a
finding rather than an oversight:

1. **dp-swap** (`crates/brocanth-cli/vendor/dpswap.min.js`, commit `f80aeeb`).
   It has no mutating verb. Any answer that needs a POST is blocked on the
   server half that PLAN.md §5 reserves.
2. **rsx**. Templates compile into the binary. There is no client-side
   component runtime, so nothing that needs one is reachable at any price.
3. **The module system** (`crates/brocanth-scaffold`). A capability reaches a
   user through `brocanth create` or `brocanth add`. A capability with no
   `Module` is a library, not a feature.

---

## Part 1 — what the five frameworks provide

### 1.1 Astro

**Version checked: Astro 7.4, released 2026-10-15.** Docs at
`docs.astro.build`; release notes for 7.0 (2026-06-22) and 6.0 (2026-03-10).
Astro 7 replaced the Go `.astro` compiler with a Rust one, made Sätteri the
default Markdown processor, and promoted `advancedRouting`, `cache`, `logger`
and `queuedRendering` out of `experimental`.

| # | Capability | What it needs at runtime |
|---|---|---|
| A1 | `.astro` component format: a `---` frontmatter fence, TypeScript and `await` above it, markup below, props through `Astro.props` typed by a local `interface Props` | Build time or server; the component never ships to the browser |
| A2 | `<slot />`, named slots (`<slot name="x" />`), fallback content, `Astro.slots.has()` / `.render()`, `<Fragment>` for no wrapper | Build time |
| A3 | Client islands: `client:load`, `client:idle`, `client:visible`, `client:media`, `client:only="svelte"` on an imported framework component | A client-side component runtime per island |
| A4 | Official renderers `@astrojs/react`, `-preact`, `-vue`, `-svelte`, `-solid-js`, `-alpinejs`, registered in the integrations array | Server render always; client runtime only under a `client:*` directive |
| A5 | Server islands: `server:defer` on any `.astro` component, with `slot="fallback"` for the first response | A server adapter **and** client JS: the island is fetched from `/_server-islands/...` after load. Props are encrypted into the URL, keyed by `ASTRO_KEY` |
| A6 | File-based routing in `src/pages/`; `_`-prefixed files excluded | Build-time route table |
| A7 | Dynamic params `[param]` and rest params `[...slug]` | Build-time table; matched at build or per request |
| A8 | `getStaticPaths()` returning `{ params, props }` — required for a prerendered dynamic route | Pure build time |
| A9 | `paginate()` inside `getStaticPaths`, giving `page.data`, `page.url.prev/next`, `currentPage`, `lastPage` | Build time |
| A10 | Per-route `export const prerender = false` over `output: 'static' \| 'server'`; mixed static and server in one build is the default | Decides build time against request time |
| A11 | Adapters through `astro add`: `@astrojs/node`, `-cloudflare`, `-netlify`, `-vercel`. They unlock `Astro.request`, `.cookies`, `.locals`, `.response` and HTML streaming | Server |
| A12 | Layouts as plain `.astro` components wrapping `<slot />`, nestable; a Markdown file picks one with the `layout` frontmatter key and gets a `frontmatter` prop | Build time |
| A13 | Content collections: `src/content.config.ts`, `defineCollection()`, `glob()` / `file()` loaders, Zod schemas, `reference()`, queried with `getCollection()` / `getEntry()` / `render()` | Build time; content is baked into a build-time store |
| A14 | Live content collections: `src/live.config.ts`, `defineLiveCollection()`, `getLiveCollection()` | Server; fetched per request |
| A15 | Route caching: `cache: { provider: memoryCache() }`, `routeRules`, `Astro.cache.set()` with `maxAge` / `swr` / `tags`, `cache.invalidate({ tags, path })` | Server with a tag index. CDN providers `cacheNetlify()` etc. are still experimental |
| A16 | Actions: `src/actions/index.ts`, `defineAction({ input, handler, accept: 'form' })`, called as `actions.foo()`, read back with `Astro.getActionResult()`, errors through `ActionError` / `isInputError()` | Server. `<form method="POST" action={actions.foo}>` works with **zero client JS**; the typed RPC client needs JS |
| A17 | Sessions: `Astro.session` with `get` / `set` / `destroy` / `regenerate`, Unstorage drivers, `App.SessionData` for types | Server, no client JS |
| A18 | Middleware: `src/middleware.ts` exporting `onRequest(context, next)`, `context.locals`, `sequence()`, `context.rewrite()` | Build time for prerendered routes, per request for the rest |
| A19 | Endpoints: `.ts` in `src/pages/` exporting `GET` / `POST` / `ALL`, typed `APIRoute`, returning a `Response` | Build time for static file endpoints, server for the rest |
| A20 | Advanced routing: `src/fetch.ts` default-exporting `{ fetch(request) }`, composed from `astro()`, `FetchState`, `actions()`, `middleware()`, `pages()`, `i18n()`. Stable in 7.0; a Hono app can replace it | Server |
| A21 | View transitions: `<ClientRouter />` from `astro:transitions`, `transition:name` / `:animate` / `:persist`, the five `astro:*` lifecycle events, `navigate()` | **A client-side JS router, always.** Incompatible with `security.csp` |
| A22 | Prefetch: the `prefetch` config with `defaultStrategy` of `hover` / `tap` / `viewport` / `load`, the `data-astro-prefetch` attribute, `prefetch()` | Client JS, but a small script and not a component runtime |
| A23 | Images: `<Image />`, `<Picture />`, `getImage()` from `astro:assets`, Sharp by default, `image.domains` / `remotePatterns`, responsive `layout` of `constrained` / `full-width` / `fixed` | Build-time transforms for prerendered pages, request-time for the rest. No client JS |
| A24 | Fonts: the `fonts` config array with `fontProviders.google()` / `.local()` / `.fontsource()` and `<Font />` with `cssVariable` and `preload`; generated fallbacks | Build time: downloads, subsets, emits CSS and preload links |
| A25 | Scoped `<style>` by default, `is:global` / `:global()` to escape, `define:vars` to pass server values, `scopedStyleStrategy` | Pure build time; scoping is a compile-time attribute rewrite |
| A26 | Processed `<script>`: bundled, TypeScript-compiled, deduplicated across repeated components, small ones inlined; `is:inline` opts out | Client JS, plain scripts. This is Astro's no-framework interactivity path |
| A27 | `astro:env`: `envField.string({ context: 'client' \| 'server', access: 'public' \| 'secret' })`, read from `astro:env/client` or `/server`, plus `getSecret()`. A secret marked client is rejected | Build-time validation; secrets never reach the bundle |
| A28 | i18n routing: the `i18n` config with `locales`, `defaultLocale`, `routing.prefixDefaultLocale`, `fallbackType`, `domains`; `getRelativeLocaleUrl()`, `Astro.currentLocale`, `Astro.preferredLocale` | Routing is build time; `domains` and `preferredLocale` need a server |
| A29 | CSP: `security: { csp: true }` with `algorithm`, `directives`, `styleDirective` / `scriptDirective` taking `hashes` and `resources`. Astro hashes its own bundles | Hash-based, so it works on fully static output with no server |
| A30 | Integrations API: twelve `astro:*` hooks plus `injectRoute()`, `injectScript()`, `addRenderer()`, `addClientDirective()`, `addMiddleware()`, `addDevToolbarApp()`. Installed with `astro add <name>`, which edits the config file | Build-time plugin system over Vite |
| A31 | Dev toolbar: Astro Menu, Inspect (every island, its props, its directive), Audit (performance and a11y), Settings; extended with `defineToolbarApp()` | Dev only; never in the build output |
| A32 | `astro check`, `astro sync` generating `.astro/types.d.ts`, `@astrojs/ts-plugin`, `ComponentProps` and `HTMLAttributes`, `App.Locals` augmentation | Build and CI time |
| A33 | Agent-facing CLI in 7.x: `astro dev --background` with `astro dev stop/status/logs` and a `/_astro/status` endpoint, auto-enabled when a coding agent is detected; `--json` structured logs; `astro build --timings` | Dev tooling |

**Experimental in 7.4:** `clientPrerender`, `contentIntellisense`,
`chromeDevtoolsWorkspace`, `svgOptimization`, `collectionStorage`,
`incrementalBuild`, `parallelPrerender`, `redirectPage`, the Container API, and
the CDN cache providers.

**The reading that matters here.** Only three Astro capabilities require a
client-side component runtime: islands (A3/A4), the `<ClientRouter />` (A21),
and the fetch half of server islands (A5). Everything else is build time or
server side. Astro's actions (A16) have a genuine no-JS path through a plain
form POST.

### 1.2 Next.js

**Version checked: Next.js 16.4, released 2026-10-06.** Docs at
`nextjs.org/docs`, pages stamped 2026-06-02 to 2026-10-15; 16.0 release notes
(2025-10-21) carry most of the model change. Ships React 19.3.

Two caveats the matrix must carry. The App Router has **two** caching models:
the legacy one, and Cache Components (`cacheComponents: true`), which
`create-next-app` now enables by default and which becomes the only one in
Next 17. And several named APIs are gone: `middleware.ts` is deprecated for
`proxy.ts`, `experimental.ppr` is removed, and `dynamic` / `revalidate` /
`fetchCache` segment configs are removed under Cache Components.

| # | Capability | What it needs at runtime |
|---|---|---|
| N1 | Nested layouts: `layout.tsx` per segment, preserved across navigation, DOM not remounted; the root layout renders `<html>` and `<body>` | A client router that swaps a sub-tree without a document load |
| N2 | `template.tsx`: same position, a new instance per navigation, effects re-run | Client remount semantics |
| N3 | `page.tsx` makes a segment routable; `params` and `searchParams` arrive as **Promises** since 16.0 | Server render |
| N4 | `loading.tsx` auto-wraps the segment in `<Suspense>` with itself as the fallback | HTTP streaming plus a client runtime that replaces the fallback in place |
| N5 | `error.tsx`: a per-segment error boundary, `'use client'`, props `error` (redacted, with `error.digest`) and **`retry()`** (stable since 16.3) | A client runtime that re-requests one segment |
| N6 | `global-error.tsx` replaces the root layout; renders its own `<html>`; gets no global styles | Document-level client boundary |
| N7 | `not-found.tsx` with `notFound()`; `forbidden()` / `unauthorized()` behind `experimental.authInterrupts` | A server-side throw-to-render, and status set before the first byte |
| N8 | `default.tsx` for an unmatched parallel slot. **Since 16.0 every slot needs one or the build fails** | Server-side slot resolution |
| N9 | Parallel routes `@slot`: passed to the layout as named props, no URL effect, each streams independently. **Every slot renders server-side even when the layout discards it** | Multi-root rendering plus per-slot navigation state |
| N10 | Intercepting routes `(.)` / `(..)` / `(...)`: soft navigation renders in place as a modal, hard navigation renders the real page | A client router that tells soft from hard, plus History API URL masking |
| N11 | Route groups `(name)`: organise folders with no URL segment; can give a sub-tree its own root layout | Build time |
| N12 | `route.ts` handlers exporting `GET` / `POST` / …, over Web `Request` / `Response`, extended by `NextRequest` / `NextResponse`; `RouteContext<'/users/[id]'>` is a generated type | A plain HTTP handler — the one entry with no client-runtime requirement |
| N13 | `proxy.ts` (was `middleware.ts`): one root file, `export const config = { matcher }`, runs on Node before the request completes; can rewrite, redirect, set headers or respond | A request interceptor in the server pipeline |
| N14 | Server Components by default, rendering to an **RSC Payload** — a serialized tree with client placeholders and their props. Later navigations fetch the payload alone | A wire format the client reconciles against. The structural dependency behind almost everything else |
| N15 | `"use client"`: the module-graph boundary. Server Components passed as `children` stay on the server. `server-only` / `client-only` fail the build on misuse; only `NEXT_PUBLIC_` env vars reach the browser | A bundler that splits by directive, plus a client React runtime |
| N16 | `"use server"` / Server Functions. **"Actions use the POST method, and only this HTTP method can invoke them."** Dispatched one at a time. Reachable by direct POST, so each does its own authz | **A mutating POST. No GET fallback exists** |
| N17 | Server Actions in forms: `<form action={serverFn}>` receiving `FormData`. In a Server Component the form **submits before JS loads and with JS off** | Native form POST for the no-JS path; the client runtime for the single-roundtrip path |
| N18 | `useActionState(action, initialState)` returning `[state, formAction, pending]` — the recommended way to surface pending state and return expected errors as values | Client React runtime over the POST |
| N19 | Revalidation from a Server Action: `updateTag(tag)` (read-your-writes), `refresh()` (uncached data only), `revalidatePath()`, `revalidateTag(tag, profile)`, `redirect()` | A server cache with tag invalidation, and a response channel carrying a fresh payload back in the POST response |
| N20 | `'use cache'` on a function, component, page or file. The key is build ID + a hash of location and signature + serialized arguments **+ variables captured from enclosing scope** | A compiler that derives keys from closure capture, and a content-addressed store |
| N21 | `cacheLife(profile)` with three independent windows — `stale` (browser), `revalidate` (server), `expire`. The client router enforces a 30-second minimum stale time regardless of config | A three-tier TTL shared between server store and browser |
| N22 | `cacheTag(tag)`, invalidated by `revalidateTag(tag, profile)` — the second argument is now required | A tag index over the cache store |
| N23 | Three cache stores: prerendered output on disk or CDN; a shared server store (in-memory LRU, swappable through `cacheHandlers`, or `'use cache: remote'`); and the browser, where `'use cache: private'` lives and is the one cached form allowed to read `cookies()` | A pluggable KV store with TTL and tags, plus a browser payload cache. Build ID is in every key, so nothing survives a deploy |
| N24 | Partial Prerendering, now the default under Cache Components rather than a flag. One response carries a static shell and streams the request-time holes into it. `cookies()` no longer makes the whole route dynamic | **Out-of-order streaming into a prerendered document** |
| N25 | ISR and the App Shell: `generateStaticParams` prerenders the listed URLs; any other URL gets the reusable App Shell instantly, then upgrades in the background and caches for the next visitor | Background regeneration plus a write-back cache |
| N26 | `ensureStatic` (new in 16.4): `'navigation'` **fails the build** if any dynamic content can reach the route; `'prefetch'` and `'shell'` are narrower | Build-time static analysis of the render tree |
| N27 | Streaming as an enforced contract: an uncached async read must sit behind `<Suspense>` or inside `use cache`, or the build can fail. `Math.random()` needs `await connection()` or `use cache` | Streaming plus framework-level static analysis |
| N28 | `navigation()` and `prefetch()` from `next/cache` (16.4): exclude a subtree from prefetch, or from the shell | The renderer runs the same tree at three stages |
| N29 | `next/link` prefetching: fires on viewport entry, re-fires on hover when stale, cancels on exit, production only. 16.0 added **layout deduplication** (one shared layout downloaded once across 50 links) and incremental prefetching | **GET only** — IntersectionObserver plus GET fetches of payloads, and a client payload cache. The one large subsystem needing no mutating verb |
| N30 | `next/form`: `<Form action="/search">` submits as a **GET** that updates search params through client navigation, and prefetches the destination | GET plus the client router |
| N31 | `generateMetadata` and the static `metadata` export; streaming metadata injects `<head>` after resolution rather than blocking | Late `<head>` injection into a streaming document |
| N32 | Bot detection: streaming metadata is disabled by User-Agent for crawlers, configured by `htmlLimitedBots`; under PPR such bots skip the shell and get a full dynamic render | A UA-keyed branch between two rendering strategies |
| N33 | File-based metadata: `opengraph-image.*`, `icon.*`, `robots.txt`, `sitemap.xml`, each with a `.tsx` variant; `generateImageMetadata`, `generateSitemaps` | Build-time file-to-route mapping |
| N34 | `ImageResponse` from `next/og`: JSX and CSS to PNG through Satori and resvg, flexbox subset only | An in-process layout and raster engine |
| N35 | `next/image`: `srcset` against `/_next/image?url=&w=&q=`, format negotiated from `Accept`, `placeholder="blur"`, `fill`, `sizes`, `loader`. Security through `remotePatterns` and `localPatterns`; `dangerouslyAllowLocalIP` blocked by default | An image-processing endpoint with its own cache |
| N36 | `next/font`: Google fonts downloaded at build and self-hosted, **no browser request reaches Google**; `adjustFontFallback` synthesizes a metric-matched fallback for zero CLS | Build-time fetching, subsetting and metric extraction; zero runtime cost |
| N37 | **No built-in App Router i18n routing.** The documented path is manual: detect in `proxy.ts`, nest under `app/[lang]/`, and use `next/root-params` so server code reads the locale without prop drilling | Request interception plus dynamic segments |
| N38 | Turbopack by default for `next dev` and `next build` since 16.0; `--webpack` opts out. Filesystem disk cache, lazy server HMR, a single shared runtime chunk | Build tooling |
| N39 | `instrumentation.ts`: `register()` once per server instance, and `onRequestError` whose context carries `routeType` and `renderType: 'dynamic' \| 'dynamic-resume'` — the second meaning PPR | A server lifecycle hook, plus error reporting that knows the rendering phase |
| N40 | Draft Mode: `draftMode().enable()` sets `__prerender_bypass`; a request carrying it **re-executes every `'use cache'` scope without writing back** and is excluded from the ISR cache | A per-request cache-bypass flag threaded through every cache layer |
| N41 | `after()` from `next/server`: schedule work for after the response has streamed | A post-response task queue |
| N42 | `next typegen` without a full build, so `tsc --noEmit` validates route usage in CI; `next analyze` with import-chain tracing and bundle diffs | Build tooling |
| N43 | `next upgrade --agent`, `experimental.agentUpgrade`, `experimental.agentFeedback`, the DevTools MCP server, and published Skills for Cache Components adoption | Dev tooling. All but `next upgrade` are experimental |

**The reading that matters here.** Next's mutation story has no GET fallback at
all (N16). Its prefetch story (N29) is pure GET and is the most directly
comparable subsystem to dp-swap. Its caching story (N20–N25) is a tag-indexed
store with a three-window TTL, which is a server-side problem rather than a
client one.

### 1.3 Rails

**Version checked: Rails 8.1.4.** Guides at `guides.rubyonrails.org`. Rails 8.0
(November 2024) brought the Solid stack, Kamal 2, Thruster and Propshaft; 8.1
(October 2025) added Active Job Continuations, `Rails.event`, `bin/ci`,
Markdown rendering and `rails credentials:fetch`.

| # | Capability | What it needs at runtime |
|---|---|---|
| R1 | `rails new myapp --database=postgresql` writes the tree, `Gemfile`, `bin/setup`, `bin/dev`, `Dockerfile`, Kamal config and a test harness. `--skip-*` removes whole frameworks | Build |
| R2 | `bin/rails generate scaffold post title:string body:text` emits model, timestamped migration, a controller with seven RESTful actions, five views, the `resources` route and a system test — one command | Build to emit; `create` / `update` / `destroy` are POST / PATCH / DELETE |
| R3 | `generate model`, `generate controller`, `generate resource`, and **`bin/rails destroy`**, which reverses any generator | Build |
| R4 | `generate migration AddPartNumberToProducts part_number:string` parses the `Add<Columns>To<Table>` name and writes the body | Build |
| R5 | `bin/rails generate authentication` (8.0+): `User` with `has_secure_password`, a DB-backed `Session` with `ip_address` and `user_agent`, a `Current` request-scoped singleton, an `Authentication` concern, and the password-reset mailer | Sign-in and sign-out are POST / DELETE |
| R6 | Custom generators (`generate generator`), `config.generators` to swap ORM or test framework, and `rails new app -m template.rb` | Build |
| R7 | Associations with `dependent:`, `polymorphic: true`, `counter_cache`, `touch:`, `inverse_of`; `has_many :through`. 8.1 adds `deprecated: true` with `:warn` / `:raise` / `:notify` on any use | Reads are GET-safe; `dependent: :destroy_async` needs a worker |
| R8 | Single Table Inheritance through a `type` column, and `delegated_type` as the per-table alternative | — |
| R9 | Validations: `validates :title, presence: true, length:, uniqueness:, format:, inclusion:`, plus `validate :method`, `validates_with`, `if:` / `on: :create`, and an `errors` object with `full_messages` | A failed `save` returns false, and the form re-renders with errors |
| R10 | Callbacks: `before_validation` … `after_commit` / `after_rollback` / `after_touch`, with `if:` guards. `after_create_commit` is what Turbo broadcasting hangs off | Server-internal |
| R11 | `scope :published, -> { … }` composing chainable queries; `default_scope`, `unscope`, `rewhere`, `merge`, `none` | — |
| R12 | `includes` picking a strategy, `preload` and `eager_load` forcing one, and **`strict_loading`**, which raises on any lazy association load | — |
| R13 | `enum :status, [:draft, :published]` generating `post.published?`, `post.published!` and a `Post.published` scope from an integer column | — |
| R14 | `find_each` / `find_in_batches` / `in_batches` paging with a cursor; `find_each(start:)` is what Continuations resume on | — |
| R15 | `insert_all`, `upsert_all`, `pluck`, `pick`, `explain`, `annotate`, and `load_async` running a relation on a thread pool | A thread pool, not a worker process |
| R16 | `encrypts :email, deterministic: true` at column level, keys from `db:encryption:init` into credentials; deterministic mode keeps `find_by_email` working | — |
| R17 | Multiple databases: `connects_to`, `connected_to(role: :reading, prevent_writes: true)`, horizontal sharding, and the `DatabaseSelector` middleware that **routes GET and HEAD to a replica automatically** | — |
| R18 | Reversible migrations: one `change` method, the inverse inferred, `reversible do \|direction\|` for what it cannot infer; the class name carries a version (`ActiveRecord::Migration[8.1]`), so old migrations keep old defaults | Build |
| R19 | A schema dump in two formats — `db/schema.rb` or `db/structure.sql` — with applied versions in `schema_migrations` | Build |
| R20 | `db:create`, `db:migrate`, `db:migrate VERSION=`, `db:rollback STEP=3`, `db:migrate:status`, `db:schema:load`, `db:seed`, `db:prepare` (idempotent) | Build |
| R21 | Strong parameters: `params.expect(post: [:title, :body])` in Rails 8. An unpermitted hash raises on mass assignment | POST bodies only |
| R22 | Filters: `before_action :require_login, only: […]`, `after_action`, `around_action`, `skip_before_action`; a filter that renders halts the chain | — |
| R23 | `respond_to` dispatching one action to `format.html`, `.json`, `.turbo_stream`, and in 8.1 `.md` through `render markdown:` | — |
| R24 | `flash[:notice]`, `flash.now`, `redirect_to root_url, notice: "…"`, `flash.keep`; `cookies.signed` / `.encrypted` / `.permanent`; `reset_session` | Session-backed, written across a redirect |
| R25 | **CSRF**: every non-GET form carries a hidden `authenticity_token`, and Rails rejects a mutating request without a valid one | A hard wall for a client with no POST, and the reason GET must stay side-effect free |
| R26 | Conditional GET: `fresh_when(last_modified:, etag:)` and `stale?` emitting `ETag` / `Last-Modified` and answering 304 | **Pure GET.** The single most relevant Rails feature for a GET-only client |
| R27 | `resources :photos` generating seven named routes plus `photos_path`, `photo_path(id)`, `new_` and `edit_` helpers and their `_url` variants; `only:` / `except:`, singular `resource`, `shallow: true` | `index` / `show` / `new` / `edit` are GET |
| R28 | `member do get "preview" end` and `collection do get "search" end`; `concern :commentable` for reusable route groups; `namespace` and `scope` | — |
| R29 | Constraints: segment regexes, request constraints (`subdomain:`), globs, `defaults:`, `direct :homepage` for custom helpers, `resolve` for polymorphic targets, `mount RackApp, at:` | — |
| R30 | `bin/rails routes` with `--expanded`, `-g`, `-c` and **`--unused`**; `/rails/info/routes` in the browser; `assert_generates` / `assert_recognizes` / `assert_routing` | — |
| R31 | `form_with model: @book` inferring URL and verb from `persisted?`, scoping names to `book[title]`, injecting the hidden `_method` field and the authenticity token; `f.text_field`, `f.collection_select`, `f.fields_for` | POST. The `_method` tunnel exists because browsers cannot issue PATCH |
| R32 | Partials: `render partial:, collection:, as:, locals:, spacer_template:`, underscore naming, the automatic `product_counter` local, `render @collection` shorthand, per-partial layouts | — |
| R33 | Layouts: `layout "admin"` per controller or `layout :method` at runtime, `yield :sidebar` with `content_for`, nested layouts, and template inheritance walking the controller ancestry | — |
| R34 | `accepts_nested_attributes_for :addresses` with `f.fields_for`, creating and updating associated records from one submission | POST |
| R35 | **Turbo Drive**: intercepts every link and form, fetches with `fetch`, swaps `<body>` without a teardown; `data-turbo="false"` opts out, `data-turbo-permanent` preserves an element | Link navigation is plain GET |
| R36 | **Turbo Frames**: `turbo_frame_tag dom_id(post)` scopes a navigation to one region; a link inside fetches with a `Turbo-Frame` header and replaces only the match. Lazy frames (`src:`, `loading: "lazy"`) fetch on view | **GET-driven throughout** |
| R37 | Turbo Streams over the response channel: `format.turbo_stream` with `turbo_stream.replace` / `.append` / `.prepend` / `.remove` / `.update` / `.before` / `.after` / `.refresh` | **POST.** This channel opens only on a non-GET submission; Turbo ignores stream responses to a GET |
| R38 | Turbo Streams over WebSocket: `turbo_stream_from "posts"` in the view plus `after_create_commit { broadcast_append_to("posts") }` in the model | A socket, and a worker for `broadcast_*_later_to` |
| R39 | Page refreshes with morphing: `turbo_refreshes_with method: :morph, scroll: :preserve` diffs the DOM in place; `broadcasts_refreshes` turns a commit into a refresh for every client | Morph-on-refresh is GET; broadcast refreshes need a socket |
| R40 | Stimulus: `data-controller`, `data-action="click->clipboard#copy"`, `data-clipboard-target`, `static targets` / `static values`, `generate stimulus`. No build step under importmaps | Client side only |
| R41 | Action Cable channels with `subscribed` / `unsubscribed`, `stream_from`, `stream_for`, and public methods callable from the browser through `perform()` | Socket |
| R42 | `ActionCable.server.broadcast` and `Channel.broadcast_to`; adapters `async`, `solid_cable`, `redis`, `postgresql` | Socket |
| R43 | `MyJob.perform_later(record)`, `perform_all_later`, `set(wait:, queue:, priority:)`, `queue_as`, `retry_on` / `discard_on`, GlobalID argument serialization | Worker |
| R44 | `ActiveJob::Continuable` (8.1): `step :process do \|step\| … step.advance! from: record.id end` checkpoints a long job so a restart resumes at the cursor — written for Kamal's 30-second shutdown window | Worker |
| R45 | **Solid Queue**, the default adapter in Rails 8: database-backed, `FOR UPDATE SKIP LOCKED`, configured in `config/queue.yml`, run by `bin/jobs start`. No Redis | Worker, no extra datastore |
| R46 | **Solid Cache**, the default store: database-backed with FIFO eviction | — |
| R47 | **Solid Cable**, the default pub/sub adapter: messages through the database, retained a day | Socket |
| R48 | Fragment caching with `<% cache product do %>`; the key comes from `updated_at` and the template digest, so editing a template invalidates automatically | — |
| R49 | Russian-doll caching: nested `cache` blocks plus `belongs_to :product, touch: true`, so a child save expires the outer fragment and inner ones survive | — |
| R50 | Collection caching: `render partial:, collection:, cached: true` doing one multi-read; `Rails.cache.fetch`; a per-request SQL query cache | — |
| R51 | `has_one_attached` / `has_many_attached` with no column, through `ActiveStorage::Blob` and `::Attachment`; `attach`, `download`, `purge`, `purge_later` | Upload is POST; `purge_later` needs a worker |
| R52 | Storage services in `config/storage.yml`: `Disk`, `S3`, `GCS`, `Mirror`, per environment. Files served in redirect mode (signed ID then 302) or proxy mode for a CDN | Serving is GET |
| R53 | Variants: `avatar.variant(resize_to_limit: [100, 100])`, named variants in the model, `representation` dispatching to `variant` or `preview`. Needs libvips or ImageMagick, ffmpeg, poppler | Requesting is GET; generating is a job |
| R54 | Direct upload: `file_field :attachments, direct_upload: true` plus `ActiveStorage.start()`, browser to S3, bypassing the app server; needs bucket CORS | POST to the storage service |
| R55 | Action Mailer: `generate mailer`, `mail(to:, subject:)`, automatic multipart when both `.html.erb` and `.text.erb` exist, `attachments.inline[…]`, interceptors and observers, `deliver_now` against `deliver_later` | SMTP; `deliver_later` needs a worker |
| R56 | **Mailer previews** at `/rails/mailers`: preview classes render the real template in the browser with no mail sent | A genuinely GET-only developer surface |
| R57 | Action Text: `has_rich_text :content` on a polymorphic table with no column, the `rich_textarea` helper over Trix, sanitised `to_s`, `to_plain_text`, embedded images through direct upload | Editing is POST; rendering is GET |
| R58 | Action Mailbox: routing in `ApplicationMailbox.route` by recipient regex, `generate mailbox replies` with a `process` method, ingress adapters, a state machine with 30-day incineration, and the conductor at `/rails/conductor/` | Inbound POST from the provider, plus a worker |
| R59 | Active Model giving a plain class the model contract: `Attributes`, `Validations`, `Errors`, `Dirty`, `Callbacks`, `Naming`, `Conversion`, `SecurePassword`, `Translation`. A form object works with `form_with` and no table | — |
| R60 | `ActiveSupport::Concern` with `included do` and `class_methods do`, plus dependency resolution | — |
| R61 | Core extensions: `blank?` / `present?` / `presence`, `try`, `with_options`, `delegate`, `class_attribute`, `String#squish` / `pluralize` / `parameterize`, `Hash#deep_merge`, `1.day`, `3.megabytes`, `to_fs(:currency)` | — |
| R62 | `ActiveSupport::CurrentAttributes` for a thread-safe per-request singleton reset between requests; `ActiveSupport::Notifications` instrumenting every framework event | — |
| R63 | Structured events (8.1): `Rails.event.notify("user.signup", user_id: 123)`, `Rails.event.tagged`, `set_context`, and subscribers receiving name, payload, tags, context and `source_location` | — |
| R64 | `bin/rails credentials:edit` decrypting into `$EDITOR` and re-encrypting on save, `config/master.key` or `RAILS_MASTER_KEY`, per-environment files, and in 8.1 **`credentials:fetch kamal.registry_password`** so Kamal needs no external secret store | Build |
| R65 | `config.load_defaults 8.1` pinning every behavioural default to one version, so a gem upgrade and a behaviour change are separate decisions | Build |
| R66 | **Propshaft**: fingerprints by content digest, writes `.manifest.json`, serves through `asset_path` and `stylesheet_link_tag`, precompiles with `assets:precompile`. No transpiling, no bundling | — |
| R67 | **importmap-rails**: `bin/importmap pin react` writes `config/importmap.rb`, the browser resolves bare specifiers from a generated `<script type="importmap">`. No Node. `bin/importmap audit` checks for vulnerabilities | — |
| R68 | The opt-out path: jsbundling-rails, cssbundling-rails, tailwindcss-rails, dartsass-rails, with `bin/dev` running Puma and the watcher through `Procfile.dev` | Build |
| R69 | `bin/rails console` with a sandbox that rolls back on exit; `bin/rails runner "Post.find_each(&:reindex)"`; `rails about`; `rails stats` | Build |
| R70 | Fixtures: YAML in `test/fixtures/` loaded once, reachable as `posts(:welcome)` everywhere, with ERB and label references. Each test runs in a transaction rolled back afterwards | Build |
| R71 | Four test layers: model, controller, `ActionDispatch::IntegrationTest`, and `ApplicationSystemTestCase` driving a real browser; assertions `assert_difference`, `assert_response`, `assert_emails`, `assert_enqueued_email_with` | System tests need a browser |
| R72 | `parallelize(workers: :number_of_processors)` forking per-CPU workers each with its own numbered database; `bin/rails test path:27`, `test -n /validation/`, `test:system` | Build |
| R73 | Local CI (8.1): `config/ci.rb` with a `CI.run do step "Tests", "bin/rails test" end` DSL run by `bin/ci`, with `success?` branches and optional `gh signoff` | Build |
| R74 | `rails plugin new blorgh --mountable` creating a namespaced engine the host mounts with `mount Blorgh::Engine, at: "/blog"`; engine migrations install through a generated task | Build |
| R75 | Rack integration: `config.middleware.use` / `insert_before` / `delete`, `bin/rails middleware` printing the stack, and `mount` for any Rack endpoint | — |
| R76 | **Kamal 2** preconfigured in `config/deploy.yml`; `kamal setup` turns a bare Linux box into an app server with zero-downtime cutover. Kamal 2.8 defaults to a local registry, so a first deploy needs no account anywhere | Build |
| R77 | **Thruster** in the generated Dockerfile in front of Puma: X-Sendfile acceleration, asset caching, gzip and brotli, as one Go binary. No nginx | A pure HTTP front end |

**The asymmetry that matters here.** Turbo Frames are GET-driven and Turbo
Streams are not. A framework with no mutating verb can reproduce the whole
Turbo Frame story — scoped navigation, lazy frames, morphing refreshes — and
can reproduce neither stream channel.

### 1.4 PocketBase

**Version checked: PocketBase v0.40.5.** Still pre-1.0, and the README still
says so: full backward compatibility is not guaranteed before v1.0.0. This
repository already carries a longer treatment in `docs/pocketbase-surface.md`,
written against the same version on 2026-10-13. The table below is the short
form, for comparison against the other four.

Legend: **[GET]** reachable by a read-only client; **[MUT]** needs POST /
PATCH / DELETE; **[SSE]** needs a long-lived stream; **[SRV]** needs the server
process itself.

| # | Capability | Reach |
|---|---|---|
| P1 | One self-contained binary, `./pocketbase serve`, with `--http`, `--dir`, `--dev`, `--encryptionEnv`; `pocketbase serve yourdomain.com` gets automatic Let's Encrypt | [SRV] |
| P2 | Embedded SQLite in `pb_data/`, WAL journal and `synchronous=NORMAL` by default — and the reason it does not scale horizontally | [SRV] |
| P3 | A separate auxiliary database for logs, keeping them off the main write path | [SRV] |
| P4 | The admin dashboard at `/_/`: collection editor, record editor, log browser, settings, backups, superuser management — one embedded SPA in the same binary | [SRV] + [MUT] |
| P5 | Three collection types: `base`, `view` (read-only, backed by a SQL `SELECT` you type in, **emits no realtime events**), and `auth` | [SRV] to define |
| P6 | Fourteen field types including `relation`, `file`, `json`, `select`, `editor`, `autodate` and `geoPoint`, each with its own options | [SRV] to define, [GET] to read |
| P7 | Five API rules per collection as filter expressions: `listRule`, `viewRule`, `createRule`, `updateRule`, `deleteRule`, plus `manageRule` and `authRule`. **`null` means superusers only and is the default**; `""` means anyone | Evaluated server-side on every call |
| P8 | Request context in a rule: `@request.auth.*`, `@request.body.*`, `@request.query.*`, `@request.headers.*`, `@request.method`, and `@request.context` with one of `default` / `oauth2` / `otp` / `password` / `realtime` / `protectedFile` | `@request.body` needs [MUT] |
| P9 | Cross-collection joins in a rule: `@collection.name.field` with an `:alias` suffix for joining the same collection twice | [GET] |
| P10 | Sixteen datetime macros in filters and rules: `@now`, `@todayStart`, `@monthEnd`, `@weekday` and the rest | [GET] |
| P11 | One uniform record REST shape — `GET/POST /api/collections/{c}/records`, `GET/PATCH/DELETE .../{id}` — covering every collection from one route template | Reads [GET], writes [MUT] |
| P12 | List query parameters: `page`, `perPage`, `sort` (with `-` and the pseudo-fields `@random` / `@rowid`), `filter`, `expand`, `fields`, `skipTotal` | [GET] |
| P13 | The filter grammar: sixteen operators including `~` / `!~` for contains and the `?`-prefixed any-of family, grouped with parentheses and combined with `&&` / `\|\|`. **The same grammar serves rules and client queries** | [GET] |
| P14 | `expand` for relations, comma-separated and dot-nested, six levels deep, and only where the target's view rule is satisfied | [GET] |
| P15 | Back-relations: `comments_via_post` usable in `filter`, `sort` and `expand`, capped at 1000 records per field | [GET] |
| P16 | `POST /api/batch`: many creates, updates, upserts and deletes in one transaction, all-or-nothing. Disabled by default | [MUT]. No GET equivalent |
| P17 | Realtime over SSE: `GET /api/realtime` emits `PB_CONNECT` with a client id; the stream drops after five minutes of silence | [SSE] |
| P18 | Subscriptions set by a **second** call, `POST /api/realtime`, carrying the client id and the complete topic set. **Authorization happens on this POST, not on connect** | [SSE] + [MUT] |
| P19 | Password auth: `POST .../auth-with-password` over any unique identity field; `GET .../auth-methods` reports what a collection enables | [MUT]; the listing is [GET] |
| P20 | OAuth2: `POST .../auth-with-oauth2`, a fixed provider set listed by `GET /api/collections/meta/oauth2-providers`, redirect at `/api/oauth2-redirect` | [MUT] plus a browser redirect |
| P21 | OTP: `request-otp` then `auth-with-otp`, pending rows in `_otps`, cleaned by `__pbOTPCleanup__`. `request-otp` always returns an id, even for an unknown address — the enumeration defence | [MUT] plus SMTP |
| P22 | MFA by combining two different methods: the first returns **401 with an `mfaId`**, the second carries it; pending sessions in `_mfas` | [MUT], two round trips |
| P23 | Self-service email flows: verification, password reset and email change, each a `request-` and a `confirm-` endpoint | [MUT] plus SMTP |
| P24 | Impersonation: superuser-only, minting a **static, non-refreshable** token with a caller-chosen duration — the documented way to make a long-lived server-to-server key | [MUT] to mint, [GET] to use |
| P25 | Stateless tokens with no server-side store: `Authorization:` with no `Bearer` prefix, five token families with their own durations, **no logout endpoint**, and a signing key of `record.tokenKey` plus a per-collection secret, so a password change invalidates every token for that record | [GET] to use |
| P26 | File upload through the record API as multipart only; stored names get a ten-character random suffix; `field+` appends, `field-` removes | [MUT], multipart |
| P27 | File serving with on-the-fly thumbnails: `?thumb=WxH` and the five other forms (`WxHt`, `WxHb`, `WxHf`, `0xH`, `Wx0`), jpg / png / gif / partial webp. An unknown size returns the original | [GET] |
| P28 | Protected files: a field option, then `?token=` from `POST /api/files/token` (180 seconds), and the collection's **`viewRule`** re-evaluated with `@request.context = "protectedFile"` | [MUT] to mint, [GET] to fetch |
| P29 | Pluggable S3 storage swapped from the dashboard, verified with `POST /api/settings/test/s3` | [SRV] |
| P30 | Backups: list, create, upload, download, delete and restore over the API, with a cron schedule and `cronMaxKeep`. Since v0.40.0 they no longer lock the database | Listing [GET], the rest [MUT] |
| P31 | A request log API — `GET /api/logs`, `/{id}`, `/stats`, and `DELETE /api/logs` (new in v0.40) — filterable on `data.*` and trimmed by `__pbLogsCleanup__` | Reads [GET] |
| P32 | **Settings as a live API**: `GET` and `PATCH /api/settings` for SMTP, S3, backups, rate limits, batch, trusted proxy, logs and meta. Runtime configuration with no restart and no config file | Read [GET], change [MUT] |
| P33 | A built-in rate limiter configured in settings, with the audiences `""`, `@guest` and `@auth` | [SRV] |
| P34 | An in-process cron: `GET /api/crons`, `POST /api/crons/{id}` to run one now, four system jobs, and `app.Cron().Add()` for your own | Listing [GET], trigger [MUT] |
| P35 | The JS hook VM: `*.pb.js` in `pb_hooks/`, goja and ES5, fifteen prewarmed runtimes, hot reload. Each handler is serialized into its own program, so it closes over nothing | [SRV] |
| P36 | PocketBase as a Go framework: `pocketbase.New()`, 84 hooks in a three-phase naming convention, custom routes through `se.Router`, and `go build` for one static binary | [SRV] |
| P37 | Schema as data plus generated migrations: `--automigrate` writes a migration file for every dashboard change; `migrate create` / `collections` / `up` / `down` / `history-sync`; unapplied migrations run on `serve` | [SRV] |
| P38 | Collection management over the same REST API, including `truncate`, `PUT /api/collections/import`, and `meta/scaffolds`. **Schema is mutable at runtime by a superuser** | Reads [GET], changes [MUT] |
| P39 | A superuser CLI out of band from the API: `pocketbase superuser upsert`, and `superuser ips` to reset the allowlist. Recovers an instance with no browser | [SRV] |
| P40 | Two official SDKs, JavaScript and Dart, wrapping the REST shape plus an auth store and the realtime client | Mixed |
| P41 | Static hosting from `pb_public/` at the site root, and `GET /api/health` reporting `canBackup` | [GET] |

**What this repository already decided.** PLAN.md §12 declines dynamic
collections (P5, P38) outright — runtime schema and compile-time types cannot
both be the source of truth — along with the JS hook VM (P35),
database-stored settings (P32) and backups (P30). §9 argues that P7's rules
are not an expression language but a string compiled to SQL, and §11 takes
P37's field-id diffing. Those are settled; Part 3 does not reopen them.

### 1.5 Django

**Version checked: Django 5.2 LTS.** Docs at `docs.djangoproject.com/en/5.2`.
Django is the oldest of the five and the one brocanth borrows the most shapes
from, so this list is the closest thing here to a specification.

| # | Capability | What it needs at runtime |
|---|---|---|
| D1 | `models.Model` with field classes, `Meta`, and a per-model manager | Server |
| D2 | Migrations: `makemigrations` writes a numbered file from a model diff, `migrate` applies it, `sqlmigrate` prints the SQL, `showmigrations` lists state | Server, plus a migration history table |
| D3 | `RunPython` / `RunSQL` data migrations, with `reverse_code` | Server |
| D4 | A query set that is lazy, chainable and sliceable, with `select_related` / `prefetch_related` for the N+1 | Server |
| D5 | `full_clean()`, per-field `validators`, `clean_<field>()` and model-wide `clean()` raising `ValidationError` with a field map | Server |
| D6 | `ModelForm`: a form derived from a model, with `fields` / `exclude`, per-field widgets, and `is_valid()` populating `form.errors` | Server |
| D7 | A form rendered to HTML by the framework, re-rendered on failure with the submitted values and the errors beside each input | Server, no client JS |
| D8 | `django.contrib.admin`: `ModelAdmin` with `list_display`, `list_filter`, `search_fields`, `ordering`, `readonly_fields`, `inlines`, `actions` | Server |
| D9 | Admin auto-discovery through `admin.site.register` and `apps.py` | Server |
| D10 | `django.contrib.auth`: `User`, password hashing with a pluggable hasher, `authenticate()`, `login()`, `logout()`, `@login_required`, permissions and groups | Server |
| D11 | Sessions through a pluggable backend: database, cache, signed cookie, file | Server |
| D12 | CSRF: a per-session token, `{% csrf_token %}`, and middleware that rejects a POST without it | Server, no client JS |
| D13 | The template language: `{% extends %}`, `{% block %}`, `{% include %}`, filters, auto-escaping, custom tags | Server |
| D14 | Class-based views: `ListView`, `DetailView`, `CreateView`, `UpdateView`, `DeleteView`, `FormView`, each with a mixin chain | Server |
| D15 | URL routing: `path()` / `re_path()` in a `urlpatterns` list, `include()`, named routes, `reverse()` and `{% url %}` | Server; **resolved at run time, not checked by a compiler** |
| D16 | Middleware as a list of callables in settings, each wrapping the next | Server |
| D17 | The app registry: an installed app is a package with models, migrations, templates, static files and an `AppConfig`; `startapp` scaffolds one | Server |
| D18 | `manage.py` and custom commands: a `Command` class under `management/commands/` | Server |
| D19 | The cache framework: `cache.get/set`, a per-view decorator, template fragment caching, and backends for memcached, Redis, database and local memory | Server |
| D20 | Signals: `pre_save`, `post_save`, `pre_delete`, `post_delete`, `m2m_changed`, and custom ones | Server, in process |
| D21 | `settings.py` with `DEBUG`, `INSTALLED_APPS`, `DATABASES`, and a `DJANGO_SETTINGS_MODULE` switch per environment | Server |
| D22 | `collectstatic`, `ManifestStaticFilesStorage` for content-hashed names, and `{% static %}` | Server, build step |
| D23 | `check` and the deployment checklist (`check --deploy`), which refuses to start on some misconfigurations | Server |
| D24 | i18n: `gettext`, `.po` files, `{% trans %}`, `LocaleMiddleware`, per-language URLs | Server |
| D25 | The test client: `self.client.get()`, `assertContains`, `assertRedirects`, plus `TestCase` wrapping each test in a transaction | Server |
| D26 | `django-admin shell` / `shell_plus`, and `dbshell` | Server |
| D27 | Fixtures: `loaddata` / `dumpdata` as JSON or YAML | Server |
| D28 | `contrib.postgres`: array, range and `jsonb` fields, full-text search, trigram similarity | Server, Postgres only |


---

## Part 2 — what brocanth has

Measured by reading the checkout at `be93223`, plus the crate sizes and the
dependency graph. A crate in `Cargo.toml` is not evidence, so each row says
what was read.

### 2.1 The crates, and whether anything uses them

| Crate | Source lines | Tests | Crates that depend on it | On the `create` menu |
|---|---:|---:|---:|---|
| `brocanth` (core) | — | many | every plugin | n/a |
| `brocanth-pages` | — | yes | generated projects | always |
| `brocanth-model` | — | yes | admin-model | `model` |
| `brocanth-admin` | 5,274 | 73 | admin-model | **no** |
| `brocanth-admin-model` | 657 | 8 | — | **no** |
| `brocanth-jobs` | ~7,000 | ~70 | — | `jobs` |
| `brocanth-cache` | — | yes | — | `cache` |
| `brocanth-realtime` | — | yes | events (as a sink) | `realtime` |
| `brocanth-auth` | 8,875 | 146 | 2 | **no** |
| `brocanth-openapi` | 5,873 | 72 | **0** | **no** |
| `brocanth-schema` | 4,832 | 61 | **0** | **no** |
| `brocanth-storage` | 2,944 | 70 | 2 | **no** |
| `brocanth-policy` | 3,290 | 64 | 2 | **no** |
| `brocanth-resource` | 2,167 | 61 | **0** | **no** |
| `brocanth-events` | 846 | 14 | 2 | **no** |

> **Corrected.** The first version of this table counted *files under
> `tests/`* and reported two crates as having no tests at all. That measured
> the wrong thing: a `#[cfg(test)] mod tests` inside `src/` is invisible to
> it. `brocanth-storage` has a 1,440-line `src/tests.rs` wired at
> `lib.rs:120`, and `brocanth-events` ends in a test module. The source-line
> counts were wrong in the same direction — they included the test file while
> the row claimed there was none. Both columns are now counted across `src/`
> and `tests/`, with test lines excluded from the source count.

Three findings in that table.

**Nothing is a stub.** The seven crates I had not read are 846 to 8,875 lines
with real module-level arguments. `auth` is the largest crate in the
repository after the CLI.

**Two crates nothing depends on.** `schema` (4,832 lines) and `resource`
(2,167 lines) are built and wired to nothing. `resource`'s own documentation
says it answers `docs/pocketbase-surface.md` §2, which makes it the PocketBase
records API — written, and unreachable from any application.

**Everything is tested.** No capability crate here is short of tests. That is
worth saying because the first version of this document claimed two of them
had none, and the claim came from counting the wrong thing rather than from
reading anything.

**Nine of fifteen capability crates are not on the menu.** `brocanth create`
offers logging, config, sessions, model, cache, jobs, realtime, commands and
the rsx trio. Everything in the lower half of that table is a library an
application must wire by hand, which by the third constraint in the preamble
means it is not yet a feature.

### 2.2 Against the five inventories

| Capability | brocanth | Where |
|---|---|---|
| File-based routing, params, rest params (A6, A7, N3) | **yes** | `brocanth-pages`, `[id].rs` and `[...rest].rs` |
| Layouts, nested (A12, N5) | **yes** | `.rsx` layouts, `DP-Partial` renders from the shared layout down |
| Templates compiled, no client runtime (A1) | **yes** | rsx |
| Islands / client components (A3, A4, N8) | **no, and unreachable** | no client-side component runtime exists; constraint 2 |
| Server islands, deferred fragments (A5) | **no** | needs a mutating fetch; constraint 1 |
| View transitions / client router (A21) | **partial** | dp-swap boosts links and swaps by layout depth |
| Prefetch (A22) | **no** | dp-swap has no prefetch strategy |
| Actions / typed RPC from a form (A16, N12) | **no** | the gap dp-swap's missing `dp-post` names |
| Middleware (A18, N6, D16) | **yes** | `MiddlewareStack`, mounted at a pattern the application picks |
| Endpoints returning a Response (A19) | **yes** | `pages!` handlers |
| Sessions (A17, D11) | **yes** | the `sessions` module, cookie-backed |
| Route caching with tags (A15) | **no** | `brocanth-cache` is a key/value cache, not a route cache |
| Streaming / Suspense (N9) | **no, and blocked** | `Response` is `http::Response<Vec<u8>>` |
| ORM, lazy queries, N+1 help (D1, D4) | **yes** | `brocanth-model`, `Probe` detects N+1 |
| Migrations from a model diff (D2, D3) | **partial** | SeaORM migrations exist; no `makemigrations` diffing |
| Model validation with a field map (D5) | **yes** | `#[validates]`, `Errors::entries()` |
| ModelForm, re-rendered with errors (D6, D7) | **yes** | `brocanth-admin`'s `Form`, built from the schema |
| Admin with list/filter/search/ordering (D8) | **yes** | `brocanth-admin`, plus `Panel` for tabs no model owns |
| Admin auto-discovery (D9) | **partial** | discovery reads the entity registry; entities are named once in `admin_source!` |
| Auth: hashing, login, permissions (D10) | **yes, unwired** | `brocanth-auth`, 8,875 lines, not on the menu |
| CSRF (D12) | **unknown** | not audited; `auth` is the likely home |
| Class-based CRUD views (D14) | **yes, unwired** | `brocanth-resource`, nothing depends on it |
| Named routes, `reverse()` (D15) | **better** | route helpers are generated and checked by the compiler |
| App registry, `startapp` (D17) | **yes, and further** | the module system plus `brocanth add` |
| `manage.py` custom commands (D18) | **yes** | `brocanth-command`, `#[cmd]` registers at link time |
| Cache framework (D19) | **partial** | key/value only; no per-view or fragment caching |
| Signals (D20) | **partial** | `brocanth-events` is tested and `brocanth-realtime` subscribes; `brocanth-model` publishes nothing |
| Settings per environment (D21) | **yes** | `brocanth-config`, layered TOML |
| Content-hashed static files (D22, A22) | **yes** | `brocanth-assets`, `assets::url()` |
| Deployment checks that refuse to start (D23) | **partial** | a plugin's `configure` can refuse; no `check --deploy` |
| i18n (D24) | **no** | nothing in the tree |
| Test client with transactional tests (D25) | **partial** | `Harness` wraps a database; no HTTP test client |
| Shell / dbshell (D26) | **no** | |
| Fixtures (D27) | **no** | |
| Background jobs with retries and a dead letter | **yes, and further** | `brocanth-jobs`; Rails and Django both need a second product for this |
| Single binary, embedded database (PocketBase) | **branch** | `brocanth-pgserver`, unmerged |
| Realtime subscriptions (PocketBase) | **broker only** | no transport; blocked on the buffered `Response` |
| OpenAPI document (PocketBase) | **yes, unwired** | `brocanth-openapi`, 5,873 lines, one test file |

---

## Part 3 — the gaps, ranked

Ranked by what unblocks the most, not by size.

**Status, 2026-10-22.** Four of the nine have moved since this was written,
and one row in the original is now wrong about our own code. The table is
the current reading; the sections below are the original argument, kept
because the argument is still why each one matters.

| | Gap | State |
|---|---|---|
| G1 | the response body cannot stream | **done**: `Body::{Buffered,Streaming}`, `ResponseHook` split into `on_headers` and `on_body`, `StreamGate` in the shutdown |
| G2 | dp-swap has no mutating verb | **done**: `dp-post`/`put`/`patch`/`delete` in `brocanth_middleware::dp::SCRIPT`, typed `DP-*` response headers, `Oob`, `DpSwap` (`Vary`), `CsrfToken::meta_tag`, `brocanth_input::rerender` for the 422. The jobs panel's buttons use it |
| G3 | capability crates are not reachable | **mostly done**: thirteen modules are on the menu. `admin` and `admin-model` are not, and are blocked on the same slot `resource` is |
| G4 | the model layer publishes to nobody | open |
| G4b | slots declared and never read; `Ctx` has no extractor | **partly**: `GitIgnore`, `AgentsSections`, `PagesItems` and the new `CommandsMod` are read now, and `Dep::feature` is finally written into the manifest. `MiseTasks` is read now too, so three remain: `CargoExtra`, `AppItems`, `AppBeforeRouter`. `Ctx` carries the request **and the plugin context** now, read with `Ctx::provided`; `mount_routes` is still untouched. See `docs/modules.md` §6 and §7 |
| G5 | `resource` and `schema` are wired to nothing | **half**: `validation` reaches applications, `resource` is on the menu and blocked |
| G6 | no HTTP test client | **done**: `brocanth-test`, with `TestClient` and an HTML reader |
| G7 | no migration diffing | open |
| G8 | the cache is key/value only | open; G1 no longer blocks the route-cache half |
| G9 | i18n, fixtures, a shell, prefetch, `check --deploy` | open |
| G10 | **no middleware anybody can name** | **started**: `brocanth-middleware`, five hooks. See `docs/middleware.md` |
| G11 | **a handler cannot read a body** | **done**: `Ctx` carries the request, `brocanth-input` turns it into a struct. See `docs/input.md` |
| G12 | **the document cannot say what a page takes** | **done**: the handler's signature says it, `pages!` emits the table, and the same table is a command-line client. See `docs/surface.md` |
| G13 | **a template engine is a special case, not a module** | **done**: `Slot::PagesRenderers`, and `jt` attached through it with a manifest, a dependency and one block. A project can hold two engines over one page tree. Sixteen `rsx` branches remain in `create.rs` for seams that do not exist yet. See `docs/modules.md` |
| G14 | **a generated project's own tests were never run** | **done**: `the_scaffold_compiles` ran `cargo build`, so `--with openapi` shipped a failing `cargo test` from its first day. Now `cargo test`, and `Slot::PagesRoutes` carries a module's pages into the table's test. See `docs/modules.md` §6b |

**G11 was worse than the audit said.** `docs/middleware.md` §4 called form
decoding "the real gap" and described a POST handler reading `Vec<u8>` and
parsing it by hand. Reading the generated code showed it could not do even
that: `Ctx` had no body, and `dispatch` dropped the query string, the
headers and the bytes on the floor. `brocanth-schema` was stranded by the
same hole, because `Validation::check` takes a value a generated project
had no way to make.

**G10 is new, and it is the one this audit missed.** Part 1 compared
brocanth against five frameworks and did not ask what every one of them
also ships: the dozen pieces of request plumbing that are nobody's feature
and everybody's afternoon. Gorilla is the clearest inventory of that set,
which is why it is written up separately.

Reading the code for it turned up the finding under the finding: **the
hook pipeline had no production implementation at all.** `RequestHook`,
`AsyncRequestHook`, `ResponseHook` and `ErrorHook` are specified, ordered
and tested, and every `impl HookSource` in the repository was a test
fixture. A seam with no users is a seam nobody has been wrong about yet.

### G1. The response body cannot stream

`plugin_hooks.rs` declares `pub type Response = http::Response<Vec<u8>>`, and
`ResponseHook::on_response` is specified to see a complete body.
`brocanth-server`'s own `Response` is buffered too.

**Blocks:** SSE and WebSocket (`brocanth-realtime` has a broker and no
transport), Next.js streaming and Suspense (N9), Astro's server islands (A5),
and any large download that should not be held in memory.

**Cost:** three changes, enumerated in `brocanth-realtime/src/lib.rs`. A body
that can be a stream; `ResponseHook` split into a header hook that runs for
streams and a body hook that does not; ADR 2's two-phase shutdown taught about
open streams. WebSocket needs three more on top, including an HTTP upgrade,
which is a second exit from `brocanth-server`.

**Depends on:** nothing. It is the bottom of the stack.

### G2. dp-swap has no mutating verb

The bundle has `dp-get` and no `dp-post`. PLAN.md §5 says this is deliberate
and lists what the server half owes: form serialisation, CSRF injection, the
422-with-errors re-render path, response-header control, out-of-band emission,
and `DP-Partial` as a rendering mode.

**Blocks:** every interactive write in every interface built here. Today the
admin's delete, the jobs panel's retry and every form reload the whole page.
It also blocks Astro actions (A16) and Next.js server actions (N12), which are
the same idea.

**Cost:** PLAN.md §5 items 1–4. The rendering mode (§5 item 4) already falls
out of the route table and the layout chain and is described there as the
cleanest thing in the design.

**Depends on:** nothing, though out-of-band emission over SSE wants G1.

**Status: done.** The client is `crates/brocanth-middleware/assets/dpmutate.js`,
exported as `brocanth_middleware::dp::SCRIPT`. It runs beside dp-swap and
uses dp-swap's own swap, out-of-band and settle functions. The server half:

| §5 item | Where |
|---|---|
| 1. form serialisation, CSRF | the script sends the form urlencoded (multipart with a file) and the token in `x-csrf-token`, from `CsrfToken::meta_tag` or the cookie. `Csrf` already read that header |
| 1. 422 re-render | `brocanth_input::rerender(&faults, form)` and `FieldErrors`. The script swaps a 422 like a 2xx |
| 2. response headers | `dp::Directive`: `Redirect`, `Refresh`, or `Swap` with retarget, reswap and trigger. One enum, so a redirect cannot carry a retarget |
| 3. out-of-band | `dp::Oob`, which dp-swap already applied by `dp-swap-oob` |
| 4. `DP-Partial` | `html!` already rendered it; `DpSwap` adds `Vary: DP-Request, DP-Partial` and puts `DpRequest` on the request state |

Tested through `TestClient` in `crates/brocanth-middleware/tests/dp.rs`.
The jobs panel's row buttons are the first consumer.

**Still open.** The admin's layout loads dp-swap and not the script, so
the admin's own delete still reloads; the jobs panel inlines it in its tab
strip. The script has no JS test harness: the repository has none, and it
is checked by `node --check` and the HTML-level tests. The generated
project's `holding()` still parses `DP-Partial` itself instead of calling
`dp::partial_depth`.

### G3. Nine capability crates are not reachable

`auth`, `policy`, `resource`, `events`, `schema`, `storage`, `openapi`,
`admin` and `admin-model` have no `Module`. An application wires each by hand
from its documentation.

**Cost:** one `src/scaffold.rs` per crate, behind an off-by-default `scaffold`
feature, plus one line in the CLI's registry. `brocanth-cache`,
`brocanth-jobs` and `brocanth-realtime` are three worked examples.

**Depends on:** nothing, and this is the highest ratio of reach to effort in
the list. It is also the one place where parallel work collides: every module
touches `create.rs`, `modules/mod.rs` and all ten golden files.

### G4. The model layer publishes to nobody

**This section replaces a wrong one.** It claimed `storage` and `events` had
no tests. Both have plenty — 70 and 14 — and `brocanth-realtime` already
implements `ChangeSink` for `Broker`, so Django's signals (D20) are present
as a behaviour and not only as a type.

The real gap is narrower and more useful. **`brocanth-model` holds no sink.**
The publisher that PLAN.md §10 names — "the model layer publishes, realtime
subscribes" — is the end that is not wired. The subscriber half works and is
tested; a row written through the ORM reaches nothing.

**Cost:** a sink on the model layer's write path, and a way for an
application to put the broker in it. The receiving side needs no change.

**Depends on:** nothing. It also decides whether `brocanth-events` ever
deserves a scaffold module: today a module could only generate a sink with no
publisher, which is why it has none.

### G4b. Eight slots are declared and never read, and the context does not reach a handler

Two findings from writing five modules at once, both measured.

**`create.rs` consumes seven of fifteen `Slot` variants.** `CargoExtra`,
`AppItems`, `AppBeforeRouter`, `PagesItems`, `GitIgnore`, `MiseTasks`,
`NodeDependencies` and `AgentsSections` are never consulted. A module writing
into one is recorded by `Scaffold` and discarded by the generator, silently —
which already happened: one module wrote a `.gitignore` line believing it was
the slot's first user, and it was, and the line goes nowhere.

**A generated project's `Ctx` has no extractor.** A plugin declaring
`ProvidesContext<T>` publishes to a per-request context that `src/pages/mod.rs`
does not expose. `cx.extract()` works only inside `plugins::announce!` at
startup. Three generated modules told the developer to call it from a handler;
that line does not compile. Corrected in the generated prose, but the gap is
the `Ctx` type, not the prose.

**`mount_routes` never appears in generated code either.** `RouteSource` is
the seam `plugin_routes.rs` opens by naming an OpenAPI plugin and an admin
plugin as its cases. `app_rs` does not emit a mount and `dispatch` consults
only the page table, so a route bundle reaches tests and no application the
CLI can write. This is what blocks `brocanth-resource` from the menu.

**Cost:** read the eight slots, or delete them. Give `Ctx` the plugin context
and an extractor. Add a slot for mounting route bundles after `build()`.

### G5. `resource` and `schema` are wired to nothing

7,000 lines across two crates that no other crate depends on. `resource` is
the PocketBase records API by its own documentation. Either they reach
applications through G3 or they are deleted; carrying them unreferenced is
the worst of the three.

### G6. No HTTP test client

`Harness` gives a transactional database. Nothing gives Django's
`self.client.get()` with `assertContains` and `assertRedirects`. Every test in
this repository that exercises a screen calls `dispatch` directly, which is
why three rounds of tests in one afternoon asserted on CSS class names: there
was no level at which to assert on behaviour instead.

### G7. No migration diffing

SeaORM migrations are hand-written. Django's `makemigrations` reads the model
diff and writes the file. §4.3 says the generator proposes and the developer
commits, which is an argument for *writing* the file rather than applying it,
not an argument against diffing.

### G8. The cache is key/value only

No per-view cache, no fragment cache, no tag invalidation (A15, D19). The
`Cache` trait and the unlogged-table backend are the hard half; a route cache
is a `ResponseHook` away — once G1 settles what a hook sees.

### G9. Smaller, and genuinely absent

i18n (D24), fixtures (D27), a shell (D26), prefetch (A22), `check --deploy`
(D23). None blocks anything else.

### Out of scope, already decided

PLAN.md §5 refuses a LiveView-style stateful process per connection, and the
preamble's constraint 2 refuses anything needing a client-side component
runtime — which is Astro's islands (A3, A4) and Next.js's client components
(N8). §9 settles that policy rules compile to SQL rather than being an
expression language. §11 takes PocketBase's field-id diffing. Part 3 does not
reopen any of them.
