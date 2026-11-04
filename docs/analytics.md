# Analytics

`brocanth-analytics` counts page views on the server. It is the `analytics`
module. It runs no script in the page, sets no cookie, and sends nothing to
a third party. The numbers go in two tables of the database the `db` module
opens, and the admin shows them at `/admin/analytics`.

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

**No route of its own.** The module adds no page and no endpoint, so there
is nothing of its own to exclude and nothing to secure.

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

The tab shows visitors and views over 7, 30 or 90 days, a bar per day, and
the top 10 pages, referrers, browsers, systems, devices and countries. It is
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

### 7. Static sites: not supported

A static host serves the files `brocanth export` wrote and never runs this
hook. The module is not `outside_runtime`, so the static preset refuses it
through the manifest rule. That is better than shipping a counter that
counts nothing.

**Follow-up, not built:** a collector route on a brocanth server, fed by a
1×1 image or `navigator.sendBeacon` from the static pages. It would reuse
the recorder and the store; the static site would need one tag.

### 8. Enabled everywhere by default

Decision: `enabled = true` in every environment. Reason: the data stays in
your own database, and seeing the numbers move in development is the test
that the module works. A project that does not want development counts sets
`enabled = false` in `config/development.toml`.

## Settings

See [`modules/analytics.md`](modules/analytics.md) for the table. Lists
(`exclude`, `query_allow`) are file-only, like every list.

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

## Open questions

1. A collector route for static sites (decision 7): build it next, or leave
   static sites without analytics?
2. Should the dashboard offer a custom date range, or are 7/30/90 days
   enough?
3. Should `enabled` default to `false` in `config/development.toml`, as the
   robots module closes indexing there?
4. Bot detection: keep the word list, or also drop clients that never load
   a page's assets (needs a beacon, so it conflicts with "no script")?
