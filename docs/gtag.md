# Google Analytics: the `gtag` module

Status: draft for review. The module works and is tested; the choices
below are open to change.

`brocanth-gtag` and the `gtag` module put Google Analytics 4 on
a site in the way EU rules ask for: nothing reaches Google before the
visitor accepts, a refusal is as easy as an acceptance, and what is sent is
stripped of what usually identifies a person.

## `gtag` or `analytics`

brocanth has two ways to count visits. They answer different needs, and a
project may take both.

| | `analytics` | `gtag` |
|---|---|---|
| Who receives the data | your own database | Google |
| Script in the page | none | Google's tag, after consent |
| Cookies | none | Google's `_ga` cookies, after consent |
| Consent banner | none: it aims at the CNIL's consent exemption for audience measurement | required, and the module ships one |
| Static site | no: it counts on the server | yes: the banner and the tag run in the browser |
| Needs | `db` | nothing |

Choose `analytics` when first-party counts are enough. Choose `gtag` when
you need Google's reports, and accept that a banner asks every visitor.
See [analytics.md](analytics.md) for the first-party module.

## What it can and cannot guarantee

This section comes first because it is the one a reader must not misread.

**What the module does, and the tests check:**

- Before consent, the page requests no Google script. In the default
  `basic` mode, `gtag.js` loads only after the visitor clicks Accept.
- Every Consent Mode v2 signal starts `denied`. The module only ever grants
  `analytics_storage`. The three advertising signals stay `denied`, and
  Google signals and ad personalisation are off in the tag's settings.
- Refuse and Accept are two buttons of the same size and style, side by
  side. There is no pre-ticked box and no cookie wall: the page works the
  same either way.
- The choice is kept for six months by default (`consent_days`), and the
  banner can be reopened on every page to withdraw consent. Withdrawing
  removes the `_ga` cookies.
- Global Privacy Control and Do Not Track count as a refusal (`respect_gpc`).
  On a server, a request that carries either gets no banner and no tag at
  all. In a static export, the script checks the same signals in the
  browser.
- Before each page view, the URL loses its fragment and every query
  parameter not on `allow_params`. A path segment or a parameter that holds
  an `@` is removed too. The title is replaced by the path unless
  `send_title` is on. The referrer is cut to its origin.
- The admin never gets the tag. The registration reads its path from
  `src/modules/admin.rs` (`GtagConfig::exclude_path`), so a moved admin
  needs no edit.
- The tag is off until a deployment sets `enabled`, so a developer's visits
  are not counted.

**What it cannot do:**

- **It does not make the site compliant.** The site owner is the controller
  under the GDPR. They still need a privacy policy that names Google
  Analytics, a lawful basis (consent, which this module collects), and the
  data processing terms with Google, which they accept in their Google
  Analytics account.
- **It does not decide the US transfer question.** Data still goes to
  Google. Whether that transfer is lawful depends on the EU–US Data Privacy
  Framework and Google's certification under it, which the site owner must
  check. See "Transfers to the US" below.
- **It cannot catch every piece of personal data.** The scrubber removes
  what it can recognise: unknown query parameters and anything with an
  `@`. A name in a path (`/users/ann-smith`) or a phone number in a URL is
  not recognised. Keep personal data out of URLs, titles and events.
- **It does not configure the Google Analytics property.** Data retention,
  data sharing settings and user deletion are set in Google's interface.
- **It is not legal advice.** The rules differ between countries and
  change; a lawyer reads the specific case.

## Design

### A response hook, not a template

The banner and the script reach the page through a response hook
(`ResponseHook::on_body`) that inserts them before the last `</body>`.

- A hook sees every HTML page, whatever renders it: `rsx`, `jt`, a handler
  that writes a string. No renderer needs to know about analytics.
- The static export (`brocanth-static`) runs every page through the
  application's own handler, hooks included. So a static site gets the
  same markup as a server, and the module can be `outside_runtime`.
- The generator has no slot for "a line in every layout's `<head>`". This
  design does not need one, so no special case goes into `create.rs`.

The hook skips: a response that is not `200`, not `text/html`, or has no
`</body>`; a dp-swap partial (`DP-Partial` header), because the page it
goes into already has the markup; an excluded path; and, with
`respect_gpc`, a request with `Sec-GPC: 1` or `DNT: 1`. It adds
`Vary: Sec-GPC, DNT` so a shared cache keeps one copy per signal.

### A constant script and a JSON data block

The consent script (`src/consent.js`, the constant `SCRIPT`) is the same
for every site. What changes per site goes beside it in
`<script type="application/json" id="brocanth-gtag">`, which a browser
does not run. So:

- the script's `'sha256-…'` hash is a constant, and the security headers
  allow it without `'unsafe-inline'`;
- the JSON is escaped so that a setting cannot close the `<script>` it
  sits in (`<`, `>` and `&` become `<`, `>`, `&`).

The script is plain JavaScript with no dependency and no build step, about
140 lines. It also tracks dp-swap navigation: it wraps
`history.pushState` and listens for `popstate`, and sends one scrubbed page
view per navigation. `send_page_view` is off in the tag's settings, so
Google never sends an unscrubbed one.

### Consent lives in the browser

The choice is stored in `localStorage` under `brocanth-consent`, with the
time it was made. There is no server round trip, so a static site works.
The store holds only the choice and a timestamp, which is strictly
necessary to remember a refusal and does not itself need consent.

**This is not proof of consent.** GDPR Article 7(1) says that where
processing rests on consent, the controller must be able to demonstrate
that the person consented. A choice that lives only in the visitor's
browser demonstrates nothing to an authority.

### An optional consent log, for a site with a server

Set `consent_log` to a file, such as `log/consent.jsonl`. Then:

1. After each Accept or Refuse, the script sends
   `GET /__consent?c=<choice>&id=<id>&v=<version>`. The id is 16 random
   bytes the browser keeps in `localStorage`. The version is a short hash
   of the banner text.
2. The plugin appends one JSON line and answers `204`:
   `{"at":<unix seconds>,"choice":"granted","id":"…","seen":"…","banner":"…"}`.
   `seen` is the version the browser showed, `banner` the version the
   server shows now; a record whose two differ answered older text.
3. The record holds no IP address and no user agent. The id links a
   record to a browser only when that browser's owner shows it.

It is a GET so that a CSRF check, which leaves reads alone, does not
refuse it. Anything but a well-formed choice answers `400`.

It is off by default, and it cannot work on a static site: a static host
runs no code, so no one receives the report. A static site keeps the
choice in the browser only, and the doc says so rather than pretend.

The log is evidence that a browser sent a choice after it saw a given
banner text. Whether that suffices for an authority is the controller's
assessment, not the module's. Rotate and keep the file like any other
record the controller holds.

### Basic mode by default

Google's Consent Mode has two implementations:

- **Basic:** the Google tag does not load until the visitor consents.
- **Advanced:** the tag loads at once with every signal `denied`, and sends
  cookieless pings that Google uses to model the visitors who refused.

Basic is the default because only basic sends nothing to Google without
consent. Advanced is a setting (`consent_mode = "advanced"`). Under GPC or
DNT, advanced mode still loads nothing.

### The security headers

`middleware-security-headers` declares `adapts_to("gtag")`. When both
are chosen, its registration calls
`SecurityHeadersConfig::page_script(brocanth_gtag::SCRIPT,
brocanth_gtag::CSP)`. That adds the script's hash to `script-src` and
Google's origins to three directives, on top of what the file sets:

| Directive | Added |
|---|---|
| `script-src` | `https://www.googletagmanager.com` |
| `img-src` | `https://www.googletagmanager.com`, `https://*.google-analytics.com` |
| `connect-src` | `https://www.googletagmanager.com`, `https://*.google-analytics.com`, `https://*.analytics.google.com` |

The origins come from Google's CSP guide for Analytics without the
advertising features, plus `*.analytics.google.com`, where GA4 sends its
hits. The new `Csp::allow` adds sources after the base and the file's
overrides, and does not bring back a directive the file removed.

The adaptation is declared on the security headers' side because that
module writes the policy. The `gtag` module needs nothing from it.

### Settings

| Key | Default | Why |
|---|---|---|
| `enabled` | `false` | No tag outside production. Turn it on in `config/production.toml` or with `$<APP>_GTAG__ENABLED=true`. |
| `measurement_id` | empty | Required when enabled. Startup fails with a `help:` line on anything that is not `G-` and capitals and digits, which also refuses an old `UA-` id. |
| `consent_mode` | `basic` | See above. |
| `respect_gpc` | `true` | GPC and DNT count as a refusal. |
| `consent_days` | `182` | The CNIL's six months. |
| `send_title` | `false` | A title can hold a name or an order number. |
| `allow_params` | the five `utm_*` | Campaign tracking keeps working; tokens, emails and search terms do not leave. |
| `exclude` | `[]` | The project's own paths. The admin is added in code, from its module file: the backoffice is staff, not visitors. |
| `cookie_domain` | none | The domain Google's cookies live on, such as `example.co.uk`. Without it, the tag picks the domain, and withdrawal removes `_ga` cookies from the host and every parent domain (a browser ignores a public suffix such as `co.uk`). |
| `consent_log` | none | A file for the consent log above. A server only. |
| `banner.*` | English | Every string is a setting. With `i18n`, the catalogs override them per locale. |

`allow_params` and `exclude` are lists, so they live in the file only.

## Transfers to the US

- In 2022, three EU data protection authorities found that a site's use of
  Google Analytics transferred personal data to the US unlawfully: Austria
  (DSB, 13 January 2022), France (CNIL, 10 February 2022) and Italy
  (Garante, 9 June 2022). These decisions followed the *Schrems II*
  judgment, which struck down the Privacy Shield in 2020.
- On 10 July 2023 the European Commission adopted an adequacy decision for
  the EU–US Data Privacy Framework (Implementing Decision (EU) 2023/1795).
  Personal data may go to a US organisation certified under it without
  further transfer tools.
- On 3 September 2025 the EU General Court dismissed an action to annul
  that decision (case T-553/23, *Latombe v Commission*). An appeal to the
  Court of Justice was possible; check its status before relying on the
  framework.

So the 2022 decisions were made before the framework existed. The framework
stands today, and the site owner relies on it, on Google's certification,
and on Google's data processing terms. The module cannot change that
assessment; it only keeps the data Google receives small.

## What counts as personal data here

Google's own policy forbids sending it data that Google could use to
identify a person: email addresses, phone numbers, precise location. It
names the places it leaks from: page URLs and their parameters, page
titles, campaign parameters, search terms, and event fields. The module
scrubs the first three it can. It sends no event of its own other than
`page_view`, and it sets no user id.

GA4 does not log or store IP addresses, per Google. It still processes the
visitor's IP address in transit, and its cookies hold an identifier: under
EU law that is personal data, which is why consent is needed at all.

## Decisions taken

- **Translations.** With the `i18n` module, the banner follows the
  request's locale. `gtag` declares `adapts_to("i18n")`, which turns on
  the crate's `i18n` feature. The `i18n` module's hook writes
  `brocanth_i18n::Current` in the request state, and the insert hook reads
  the keys `gtag.banner.text`, `accept`, `refuse`, `manage` and `policy`
  through `current.text(key)`. A key no catalog has keeps the `banner.*`
  setting. The markup is built once per locale and kept. The banner gets
  `lang="<tag>"`, because the page around it may say another language.
  The `i18n` module writes the `[gtag.banner]` table in `en.toml` and
  `fr.toml`.
  - **Reason for a lookup by name:** the hook lives in a library and cannot
    name the application's `Locale` type, so it cannot call
    `t::gtag::banner::accept(locale)`.
  - **The consent log** records the version of the text the request's
    locale shows (`banner`), beside the one the browser saw (`seen`). Its
    request hook runs after `i18n.negotiate`.
  - **The link text** "Privacy policy" was fixed English. It is now
    `banner.policy`, so it is a setting and a catalog key like the others.
  - **A static site** gets the banner in the `default` locale of `i18n`,
    because the export sends no `Accept-Language`.

- **The wizard.** The "A static site" preset offers `gtag` in its
  operations question, unticked. A static site may count visits only in
  the browser, and Google needs consent, so the person decides.
- **Withdrawal across subdomains.** `cookie_domain` names the domain.
  Without it, withdrawal tries the host and every parent domain instead of
  guessing the last two labels, which was wrong for `example.co.uk`.
- **A server-side log of consent.** `consent_log`, off by default, as
  described above. Not possible on a static site.

## Open questions

1. **Another provider.** The banner and the scrubber do not depend on
   Google. A self-hosted counter that sets cookies (Matomo with cookies,
   for example) would reuse them with a different loader.
2. **Retention of the consent log.** The module appends and never deletes.
   Should it rotate the file, or leave that to the deployment?

## Sources

- GDPR, [Article 7](https://eur-lex.europa.eu/eli/reg/2016/679/oj), Conditions for consent:
  "the controller shall be able to demonstrate that the data subject has
  consented".

- Google, [Set up consent mode on websites](https://developers.google.com/tag-platform/security/guides/consent):
  the four v2 signals, `default` and `update`, `wait_for_update`, basic and
  advanced implementations.
- Google, [Data privacy and security in Google Analytics](https://support.google.com/analytics/answer/9019185):
  "In Google Analytics, IP addresses are not logged or stored."
- Google, [Best practices to avoid sending PII](https://support.google.com/analytics/answer/6366371):
  no data Google could recognise as PII; URLs, titles, parameters, search
  terms.
- Google, [Using Google Tag Manager with a Content Security Policy](https://developers.google.com/tag-platform/security/guides/csp):
  the origins for Analytics.
- Google, [GA4 configuration reference](https://developers.google.com/analytics/devguides/collection/ga4/reference/config):
  `allow_google_signals`, `allow_ad_personalization_signals`,
  `page_location`.
- CNIL, [Lignes directrices modificatives et recommandation (2020)](https://www.cnil.fr/fr/cookies-et-autres-traceurs/regles/cookies/lignes-directrices-modificatives-et-recommandation):
  refusing as easy as accepting, an "accept all" and a "refuse all" button,
  continuing to browse is not consent, withdrawal at any time, and keeping
  the choice for six months.
- CNIL, [Cookies : solutions pour les outils de mesure d'audience](https://www.cnil.fr/fr/cookies-et-autres-traceurs/regles/cookies-solutions-pour-les-outils-de-mesure-daudience):
  the consent exemption for audience measurement (13-month cookies,
  25-month retention, no reuse by the provider), which a default Google
  Analytics setup does not meet; hence consent.
- noyb, [Austrian DSB: EU-US data transfers to Google Analytics illegal](https://noyb.eu/en/austrian-dsb-eu-us-data-transfers-google-analytics-illegal).
- EDPB, [Italian SA bans use of Google Analytics](https://www.edpb.europa.eu/news/national-news/2022/italian-sa-bans-use-google-analytics-no-adequate-safeguards-data-transfers_en).
- Goodwin, [Use of Google Analytics by EU websites violates GDPR](https://www.goodwinlaw.com/en/insights/blogs/2022/02/use-of-google-analytics-by-eu-websites-violates-gdpr):
  the DSB (13 January 2022) and CNIL (10 February 2022) decisions.
- Norton Rose Fulbright, [Commission adopts its adequacy decision for the EU-US Data Privacy Framework](https://nortonrosefulbright.com/en/inside-fintech/blog/2023/07/european-commission-adopts-its-adequacy-decision-for-the-eu-us-data-privacy-framework).
- Clifford Chance, [Transatlantic data transfers: where the DPF stands post-Latombe](https://www.cliffordchance.com/insights/resources/blogs/talking-tech/en/articles/2025/09/transatlantic-data-transfers-where-the-dpf-stands-post-latombe.html).
- MDN, [`Navigator.globalPrivacyControl`](https://developer.mozilla.org/en-US/docs/Web/API/Navigator/globalPrivacyControl)
  and [`Sec-GPC`](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Sec-GPC).
