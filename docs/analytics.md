# Analytics

`brocanth-analytics` counts page views on the server. It is the `analytics`
module. It runs no script in its pages, sets no cookie, and sends nothing to
a third party. The numbers go in two tables of the database the `db` module
opens, and the admin shows them at `/admin/analytics`. A static site is
counted through the same server: its pages carry the `beacon` module's
snippet, and the server's collector reads what it sends.

This page records the design and the reason for each decision. Status:
draft, for review.

## What it answers

"How many people read this page this week, from where, on what kind of
device." It does not answer "what did this person do": nothing stored can
follow one person, across pages on another day, or across sites.

## Decisions

### 1. Collect in a request hook, after dispatch

One `RequestHook`, `after_dispatch`. It sees the request and the final
response, so it knows the status and the content type, and it runs after the
answer is decided. It never delays or changes a response.

A request is a page view when all of these hold:

- the method is `GET` (not `HEAD`);
- the status is 2xx, except 204 and 206;
- the response `content-type` starts with `text/html`;
- the path is not under an `exclude` entry (default `/admin`);
- it is not a prefetch: `Sec-Purpose`, `Purpose` or `X-Moz` does not say
  `prefetch`;
- the user agent does not read as a bot (a list of words in
  `classify::BOT_WORDS`; an empty user agent counts as a bot);
- with `respect_gpc`, it sends neither `Sec-GPC: 1` nor `DNT: 1`.

The content-type rule already leaves out assets, JSON and redirects, so the
module needs no list of asset paths.

**dp-swap partials count.** A navigation through dp-swap is a GET with
`DP-Partial` that replaces a full page load. Each navigation is one request,
either full or partial, so counting both counts each navigation once.

**No route of its own,** except the collector for static sites (decision
7), which is off until `collector_origins` names one. The collector answers
a 204 or an image, never HTML, so the hook never counts it.

### 2. Privacy by construction

Nothing that identifies a person is stored. Each field of a row is reduced
before it is built:

| Stored | From | How |
|---|---|---|
| `day` | the clock | the UTC date; no hour, no minute |
| `visitor` | address, user agent | see below |
| `path` | the URL | query and fragment removed; a segment with `@` or `%40` becomes `_`; 512 bytes at most |
| `query` | the URL | only the `query_allow` parameters (default the five `utm_*`), and none whose value has an `@` |
| `referrer` | `Referer` | the origin only (`https://news.example`); empty for this site's own host |
| `browser`, `os`, `device` | `User-Agent` | three family names, never a version |
| `country` | an edge header | two letters, only when `country_header` names a header such as `cf-ipcountry`; no GeoIP database |

**The visitor hash.** Unique visitors are counted the way Plausible counts
them: a hash that changes every day.

```
prehash = SHA-256(site ‖ 0 ‖ address ‖ 0 ‖ user agent)     in the hook
visitor = first 8 bytes of SHA-256(salt of the day ‖ prehash)   at write time
```

- The hook computes `prehash` at once, so the address and the user agent
  never sit in the buffer, even for a few seconds.
- The salt is 32 random bytes from the OS (`getrandom`), one per UTC day,
  kept in `brocanth_analytics_salts`. It is in the database, not in memory,
  so several instances behind one load balancer count a visitor once.
- Once a day, the flusher deletes every salt of an earlier day. From then
  on, nobody can recompute yesterday's hashes from an address to test a
  guess. The hashes become anonymous numbers.
- Within one day the hash is still pseudonymous: whoever holds the salt and
  guesses an address and a user agent can check the guess. That window is
  the price of counting a visitor once a day.

**One person on two days is two visitors.** The hash cannot link days, so
"visitors over 30 days" is the sum of daily visitors. Plausible documents
the same limit. It is the cost of a hash nobody can follow.

**GPC and DNT are counted by default.** Decision: `respect_gpc = false`.
Reason: nothing stored can identify the visitor, no data leaves the site,
and the CNIL exemption below does not ask for the signal. Skipping those
requests undercounts the audience that cares most about privacy. A site that
wants to honour the signal anyway sets `respect_gpc = true`.

### 3. Raw rows with a retention, not daily aggregates

Each page view is one row. A daily aggregate (`day, path, views, visitors`)
cannot answer "visitors to the site" from "visitors per page": a visitor who
reads two pages would count twice. Raw rows answer every breakdown with one
`count(DISTINCT (day, visitor))`. A mostly-static site writes few rows.

**Retention: 760 days (25 months) by default.** The flusher deletes older
rows once a day. It runs the delete itself, so the module needs no `jobs`
queue. `retention_days = 0` keeps rows forever, which the CNIL exemption
does not allow.

### 4. Writes never slow a request

The hook pushes onto an in-memory buffer and returns. A task writes the
buffer every `flush_interval` seconds (default 10) with one
`INSERT … SELECT FROM UNNEST(…)` per 1,000 rows.

- A failed write puts the batch back and records the error. The next tick
  tries again. The panel shows the last error.
- The buffer holds 50,000 views at most. Past that, new views are dropped
  and counted, so a database that is down for an hour cannot grow the
  process without bound.
- At shutdown, the plugin writes what is left, inside the grace period.

### 5. Storage: `needs("db")`

Decision: `needs`, not `adapts_to`. Reason: analytics that vanish at every
restart are not analytics. The crate still has a `MemoryStore` for tests and
for an application that builds its plugins in code.

The plugin finds the pool in the application's services, where the `db`
plugin put it (`after("db")`). With `migrate = true` (the default), the
first readiness check creates the two tables, as the cache and jobs modules
do. With `migrate = false`, put `PgStore::SCHEMA` in a reviewed migration.

The store uses plain SQL through `brocanth_db::Db`, not sea-query: the batch
insert is one `UNNEST` with an array per column, which sea-query has no
builder for, and every report groups by a column from a closed enum. Every
value is a bound parameter. Days travel as integers
(`DATE '1970-01-01' + n`), so the crate needs no date library.

### 6. The dashboard is an admin tab

With `admin`, the admin module writes one line in its `mount`:

```rust
config = config.panel(brocanth_analytics::admin::AnalyticsPanel::installed());
```

The tab shows visitors and views over 7, 30 or 90 days, or between any two
dates (`?from=2026-10-01&to=2026-10-31`, a GET form). A custom range is two
years at most, so one request cannot ask for a decade; a range that is
backwards, too long or unreadable falls back to 30 days. Below the totals
come a bar per day and the top 10 pages, referrers, browsers, systems,
devices and countries. It is
rendered on the server with no script. The bars are SVG `<rect>` elements
with presentation attributes, so the screen needs nothing from the
Content-Security-Policy. It is behind the admin's policy like every tab:
reading it is `Action::ViewPanel` on `analytics`.

`installed()` resolves the recorder when the tab renders, because a
generated admin mounts its routes before the plugin is built. The first
`Analytics` built in a process is the one it finds.

**Without `admin`: no endpoint.** Decision: no JSON endpoint behind a token.
Reason: a token-protected endpoint is a second authentication scheme to get
right, and the rows are already in your database. Query them with SQL:

```sql
SELECT path, count(*) AS views, count(DISTINCT (day, visitor)) AS visitors
FROM brocanth_analytics_views
WHERE site = 'my-site' AND day >= current_date - 29
GROUP BY path ORDER BY views DESC LIMIT 10;
```

### 7. Static sites: a collector on the server, a beacon in the pages

A static host serves the files `brocanth export` wrote and never runs the
hook. So `analytics` stays **not** `outside_runtime`, and the static preset
refuses it. A static site is counted by a brocanth server that has it, in
two halves:

**The collector, in `analytics`.** A second request hook on one path,
`collector_path` (default `/_a/v`). It is installed only when
`collector_origins` lists at least one origin. It reads a page view a
static page sends and counts it with the same rules as the hook:

- the visitor hash comes from the beacon request's own address and user
  agent, which are the visitor's, because the browser sends the beacon. A
  page view on the server and a beacon from one browser are one visitor;
- the page is the `p` value of the payload, reduced by `clean_path` and
  `clean_query`; the referrer is the `r` value, kept as its origin, and
  dropped when it is the static site's own;
- bots, excluded paths and (with `respect_gpc`) GPC/DNT are dropped.

Abuse, because anybody can send to a public URL:

- **Origin check.** Only an origin in `collector_origins` counts, read from
  `Origin` (`sendBeacon` sends it), or from the origin of `Referer` when
  there is none (an image request sends no `Origin`). Neither header is
  proof, and the doc says so: they keep a careless site and a casual
  script out, not a determined forger, who could also just open the pages.
- **Size.** A query or a body over 2 KiB is ignored; `clean_path` caps the
  stored path at 512 bytes.
- **Rate limits.** The path is fixed, so a `rate-limit` rule can name it,
  and the collector runs after the rate limiter when both are present.
- **One answer.** Counted or not, the collector answers 204, or a 1×1 GIF
  to a GET, with `Cache-Control: no-store`. Never a 5xx and never a reason:
  a beacon cannot read it, and a prober learns nothing.
- **CSRF.** The collector runs before the CSRF check (`before_if_present
  ("csrf.check")`): a beacon is a cross-site POST with no token, and it
  writes nothing a forged form could abuse. Every other POST still meets
  the check; a test holds both.

The path is `/_a/v` and not `/collect` or `/analytics`, which are on the
lists content blockers use. Hiding from a blocker is not the aim: a person
who blocks it has said no. The aim is not to be caught by a rule written
for third-party trackers.

**The beacon, a module of its own: `beacon`.** A response hook that puts a
`<script>` before the last `</body>` of every HTML page (at its end when
`</body>` is left out, which HTML allows; never in a dp-swap partial) once
`[beacon] collector` names a collector URL. The export runs every page
through the hooks, so every exported file carries it.

- Decision: its own top-level module, not `analytics-beacon`. Reason: the
  static site and the counting server are two projects. A static site
  cannot hold `analytics`, and a child cannot be chosen without its parent,
  so a child would be refused by the very preset it exists for. `beacon` is
  `outside_runtime`: what it adds runs at export time and in the browser.
- It conflicts with `analytics`: in one project the two would count each
  page twice.
- The script is about 600 bytes, the same text on every page; the URL and
  the GPC choice are `data-` attributes on the `<script>` element. It sends
  `p` (path and query) and `r` (`document.referrer`) with
  `navigator.sendBeacon` and a form body, a CORS-safelisted type that needs
  no preflight, or with a 1×1 image when the browser has no beacon. It
  wraps `history.pushState`, so a dp-swap navigation sends again, and
  listens to `popstate`. It sets no cookie and uses no storage; a test
  checks the text for both.
- With `[beacon] respect_gpc`, a browser with Global Privacy Control or Do
  Not Track on sends nothing.

Wiring the two: set `[beacon] collector = "https://app.example.com/_a/v"`
on the static site, and `[analytics] collector_origins =
["https://docs.example.com"]` on the server. Beacon rows carry the server's
`site`, so the server's dashboard shows the static site's pages beside its
own. Two static sites feeding one server are one site in the numbers.

### 8. Off in development, on in production

Decision: the generated `config/development.toml` writes `enabled = false`;
the plugin's default stays `true`, so production counts unless its own file
says otherwise. Reason: your own visits while you build are not the
audience, and they would sit in the numbers for 25 months. To check the
module works, set `enabled = true` locally and open `/admin/analytics`.

## Settings

See [`modules/analytics.md`](modules/analytics.md) and
[`modules/beacon.md`](modules/beacon.md) for the tables. Lists (`exclude`,
`query_allow`, `collector_origins`) are file-only, like every list.

## Privacy reasoning, and its limits

This module aims at the conditions under which the CNIL (France) exempts an
audience-measurement tool from consent:

| CNIL condition | This module |
|---|---|
| The purpose is only audience measurement, for the publisher alone | It counts views for the site that runs it; nothing leaves the site |
| It produces anonymous statistics only | The dashboard shows counts; the per-day hash becomes anonymous when its salt is deleted |
| No cross-referencing with other processing, no data to third parties | No third party; the hash includes the site, so two sites never share one |
| No tracking across sites or apps | The same |
| Tracker lifetime at most 13 months | No cookie and no stored identifier; the hash lives one day |
| Data kept at most 25 months | `retention_days = 760` by default |
| Users are informed, for example in the privacy policy | **Your job**: say in your privacy policy that page views are counted this way |

**What this does not guarantee.** This is engineering, not legal advice.

- **The site owner is the controller.** Whether the exemption applies to a
  given site is for its owner, and its regulator, to decide.
- **Article 5(3) of the ePrivacy Directive may apply without a cookie.** The
  EDPB's Guidelines 2/2023 (version 2, adopted October 2024) say that
  information a device sends automatically, such as the IP address and the
  URL, can fall under Article 5(3), and that IP-based tracking can need
  consent. The visitor hash reads the address and the user agent. This
  module therefore relies on an audience-measurement exemption, such as the
  CNIL's, and not on "no cookie means no rule". Other regulators may read
  the exemption differently or not have one.
- **Within a day, the hash is pseudonymous personal data under the GDPR**,
  because the salt still exists. The legal basis for that day is the site
  owner's choice, usually legitimate interest, with the transparency that
  requires.
- **The URL is the developer's.** The path and the allowed query parameters
  are stored. An email-shaped segment is replaced, but a name, an account
  number or a token in a path segment is not. Do not put personal data in a
  URL.
- **The bot list is a heuristic.** A bot that sends a browser's user agent
  is counted.

## Sources

- CNIL, "Cookies : solutions pour les outils de mesure d'audience":
  https://www.cnil.fr/fr/cookies-et-autres-traceurs/regles/cookies-solutions-pour-les-outils-de-mesure-daudience
  (purpose limits, 13-month tracker lifetime, 25-month retention, no
  cross-referencing, user information).
- EDPB, Guidelines 2/2023 on the technical scope of Art. 5(3) of the
  ePrivacy Directive, version 2:
  https://edpb.europa.eu/system/files/2024-10/edpb_guidelines_202302_technical_scope_art_53_eprivacydirective_v2_en_0.pdf
  (automatically sent information, IP-based tracking).
- Plausible, data policy: https://plausible.io/data-policy (the
  `hash(daily_salt + domain + ip + user_agent)` method this module follows,
  with daily salt deletion).

## Decided in review

1. Static sites: the collector and the `beacon` module (decision 7).
2. The dashboard takes any two dates as well as 7, 30 and 90 days
   (decision 6).
3. `config/development.toml` writes `enabled = false` (decision 8).
4. Bot detection stays a word list (`classify::BOT_WORDS`). Detecting a
   client that never loads the page's assets needs a script on the server's
   own pages, which this module does not ship.

## Open questions

1. Beacon rows carry the server's `site`. Should each collector origin be
   a site of its own on the dashboard?
