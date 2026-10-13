# PocketBase surface, as of v0.40.5 (2026-10-08)

Reference material gathered 2026-10-13 from `pocketbase.io/docs` and from
`github.com/pocketbase/pocketbase` source (`core/*.go`, `apis/realtime.go`,
`tools/subscriptions/*.go`, `migrations/1640988000_init.go`). Field names below
are the exact JSON keys, which is what the REST API accepts.

This file is **not a specification of what brocanth will build**. PLAN.md §12
records what is declined and in what order the rest is built. This is the thing
being compared against, written down so the comparison is to the real article
rather than to a memory of it.

## 1. Collections

Three types: `base` (a real SQLite table), `view` (read-only, backed by
`viewQuery`, **emits no realtime events**), `auth` (base plus the system fields
`password`, `tokenKey`, `email`, `emailVisibility`, `verified`, plus
`manageRule` and `authRule`).

Schema lives in a table. The five rules are **first-class nullable TEXT
columns**, not JSON:

```sql
CREATE TABLE {{_collections}} (
  [[id]]         TEXT PRIMARY KEY DEFAULT ('r'||lower(hex(randomblob(7)))) NOT NULL,
  [[system]]     BOOLEAN DEFAULT FALSE NOT NULL,
  [[type]]       TEXT DEFAULT "base" NOT NULL,
  [[name]]       TEXT UNIQUE NOT NULL,
  [[fields]]     JSON DEFAULT "[]" NOT NULL,
  [[indexes]]    JSON DEFAULT "[]" NOT NULL,
  [[listRule]]   TEXT DEFAULT NULL,
  [[viewRule]]   TEXT DEFAULT NULL,
  [[createRule]] TEXT DEFAULT NULL,
  [[updateRule]] TEXT DEFAULT NULL,
  [[deleteRule]] TEXT DEFAULT NULL,
  [[options]]    JSON DEFAULT "{}" NOT NULL,
  [[created]]    TEXT DEFAULT (strftime('%Y-%m-%d %H:%M:%fZ')) NOT NULL,
  [[updated]]    TEXT DEFAULT (strftime('%Y-%m-%d %H:%M:%fZ')) NOT NULL
);
```

Other system tables: `_params` (settings), and the system collections
`_superusers`, `_externalAuths`, `_mfas`, `_otps`, `_authOrigins`.

### Field types and their options

Common to every field: `id`, `name`, `type`, `system`, `hidden`, `presentable`,
`help` (absent on `autodate`).

| `type` | Options |
|---|---|
| `text` | `min`, `max` (0 → 5000), `pattern`, `autogeneratePattern`, `required`, `primaryKey` |
| `number` | `min`, `max` (both nullable), `onlyInt`, `required` |
| `bool` | `required` (means: must be `true`) |
| `email` | `exceptDomains`, `onlyDomains` (mutually exclusive), `required` |
| `url` | `exceptDomains`, `onlyDomains`, `required` |
| `editor` | `maxSize` (0 → ~5MB), `convertURLs`, `required` |
| `date` | `min`, `max`, `required` |
| `autodate` | `onCreate`, `onUpdate` — no `required`, no `help` |
| `select` | `values`, `maxSelect` (≤1 → string, ≥2 → array), `required` |
| `json` | `maxSize` (0 → 1MB), `required` (non-empty: not null/""/[]/{}) |
| `file` | `maxSize` (0 → 5MB), `maxSelect`, `mimeTypes`, `thumbs`, `protected`, `required` |
| `relation` | `collectionId`, `cascadeDelete`, `minSelect`, `maxSelect`, `required` |
| `geoPoint` | `required` (non-"Null Island") |
| `password` | `pattern`, `min`, `max` (0 → 71 bytes), `cost` (bcrypt), `required` |

Every field except `json` is **non-nullable with a zero default** (`""`, `0`,
`false`, `[]`). Dates are strings `Y-m-d H:i:s.uZ` and are **compared as
strings**. `created`/`updated` are NOT automatic — they are ordinary `autodate`
fields the built-in `users` collection happens to declare.

Write modifiers: `field+` / `field-` on numbers; `field:autogenerate` on text;
`field+` append, `+field` prepend, `field-` remove on select/file/relation.

## 2. Records REST API

```
GET    /api/collections/{idOrName}/records
GET    /api/collections/{idOrName}/records/{recordId}
POST   /api/collections/{idOrName}/records
PATCH  /api/collections/{idOrName}/records/{recordId}
DELETE /api/collections/{idOrName}/records/{recordId}
POST   /api/batch
```

Query params: `page` (1), `perPage` (30), `sort`, `filter`, `expand`, `fields`,
`skipTotal`. `sort` takes `-` for DESC and the pseudo-fields `@random`,
`@rowid`. `expand` goes **6 levels deep**. `fields` takes `*` at a depth and the
modifier `:excerpt(maxLength, withEllipsis?)`.

### The filter grammar

Sixteen operators:

```
=   !=
>   >=   <   <=
~   !~            Like/Contains; auto-wraps the right operand in % if none given
?=  ?!=  ?>  ?>=  ?<  ?<=  ?~  ?!~
```

The `?` prefix means **any-of**. Without it, an array-valued or multi-record
path is **match-all**. Grouping with `(...)`, `&&`, `||`. `//` line comments.

Nested paths with `.`; back-relations with `collection_via_field`, usable in
`filter`, `sort` and `expand`, capped at 1000 records per relation field.

Datetime macros, all UTC: `@now @second @minute @hour @weekday @day @month
@year @yesterday @tomorrow @todayStart @todayEnd @monthStart @monthEnd
@yearStart @yearEnd`.

Field modifiers: `:isset` (only on `@request.*`), `:changed` (only on
`@request.body.*`, equal to `(@request.body.X:isset = false || @request.body.X =
X)`), `:length`, `:each`, `:lower`. **None of them see newly uploaded files.**

Functions: `geoDistance(lonA, latA, lonB, latB)` in km, and `strftime(format,
[time, modifiers...])`.

### Batch

`POST /api/batch` with `{"requests":[{url, method, headers, body}]}`. Supports
create, update, **upsert via `PUT /api/collections/{c}/records`** (a verb that
exists only inside a batch), and delete. Per-request `Authorization` is not
supported — one auth state for the whole batch.

## 3. API rules

Five per collection — `listRule`, `viewRule`, `createRule`, `updateRule`,
`deleteRule` — plus `manageRule` and `authRule` on auth collections.

| Value | Meaning |
|---|---|
| `null` | superusers only. **The default.** |
| `""` | anyone, including guests |
| expression | only requests satisfying it |

**A rule is also a filter.** An unsatisfied `listRule` yields `200` with an
empty `items`, not `403`. `createRule` unsatisfied → `400`. `view`/`update`/
`delete` unsatisfied → `404`. A `null` rule for a non-superuser → `403`. Rules
are **ignored entirely for superusers**.

In scope inside a rule: the collection's own fields (including nested relation
paths and back-relations), `@request.context` (one of `default`, `oauth2`,
`otp`, `password`, `realtime`, `protectedFile`), `@request.method`,
`@request.headers.*` (**lowercased, `-` → `_`**), `@request.query.*`,
`@request.auth.*`, `@request.body.*` (**never uploaded files**), and
`@collection.<name>.*` with `:alias` to join the same collection twice.

## 4. Auth

Endpoints on `/api/collections/{c}/`: `auth-methods`, `auth-with-password`
(`identity`, `password`, `identityField?`), `auth-with-oauth2`, `request-otp`
(`email`), `auth-with-otp` (`otpId`, `password`), `auth-refresh`,
`request-verification`, `confirm-verification`, `request-password-reset`,
`confirm-password-reset`, `request-email-change`, `confirm-email-change`,
`impersonate/{id}`. Plus `GET|POST /api/oauth2-redirect`.

**No logout endpoint** — tokens are stateless and not stored server-side.

Five token types, all HMAC JWTs: `auth`, `file`, `verification`,
`passwordReset`, `emailChange`. The signing key is **`record.tokenKey` +
per-collection per-type secret**, so changing a record's `tokenKey` (which a
password change does) invalidates every token for that record. Default
durations: auth 432000s (5 days), passwordReset 1800, emailChange 1800,
verification 86400, file 180. Superuser auth tokens 86400.

**MFA**: `{enabled, duration (default 600), rule}`. Authenticate with method A,
get **HTTP 401 with `{"mfaId": "..."}`**, authenticate with a *different* method
B passing `mfaId`. Pending sessions are rows in `_mfas`.

**OTP**: `{enabled (default false), duration (default 180), length (default 8),
emailTemplate}`. `request-otp` **always returns an `otpId`**, even for an
unknown email — that is the enumeration defence. A successful OTP auth marks the
record `verified`.

**Impersonation**: superuser-only, issues a **static, non-refreshable** token
(`refreshable: false`) with a caller-chosen duration.

**Auth alert**: `{enabled, emailTemplate}`, backed by `_authOrigins`, fires on
login from a new device or location.

`passwordAuth: {enabled (default true), identityFields (default ["email"])}`.

## 5. Realtime

`GET /api/realtime` opens the SSE stream and immediately emits `PB_CONNECT`
carrying `{"clientId": "..."}`. Wire format, byte-exact:

```
id:<eventId>\n
event:<message name>\n
data:<raw json>\n\n
```

Subscriptions are set by a **separate** `POST /api/realtime` with `clientId`
(required) and `subscriptions` (the complete new set; empty means unsubscribe
from everything). **Authorization happens on this POST, not on connect.** The
server also checks that the subscribing IP matches the stream's IP.

Topics:

```
<collection>/*          gated by listRule
<collection>/<recordId> gated by viewRule
<collection>            deprecated alias for the wildcard
```

Per-topic options are appended as `?options=<url-encoded JSON>` with `query`
(recognised keys `filter`, `expand`, `fields`) and `headers` (which populate
`@request.headers.*` for the rule check).

Payload: `{"record": {...}, "action": "create"|"update"|"delete"}`. The SSE
`event:` name is the matching topic string including its `?options=` suffix, so
the client demultiplexes on it.

Keepalive: five minutes of silence and the server signals a disconnect; the
client reconnects and **must resubmit every subscription**.

Broadcast is to **all** connected clients in chunks — there is no topic index —
and each one costs a `SELECT (1) FROM t WHERE id = ? AND <rule>`. PLAN.md §10 is
the argument against this design, and its own source carries a `@todo` about it.

## 6. Files

Only the **filename** is stored in the row — `"test_52iwbgds7l.png"`, the
sanitized original plus a random suffix. Single file when `maxSelect <= 1`, an
array otherwise. Upload is multipart only.

```
/api/files/{collectionIdOrName}/{recordId}/{filename}?thumb=&token=&download=
```

Six thumb formats, and the string must already be listed in the field's
`thumbs`:

```
WxH    crop to the viewbox, centred
WxHt   crop from the top
WxHb   crop from the bottom
WxHf   fit inside, no crop
0xH    scale to height H
Wx0    scale to width W
```

jpg, png, gif (first frame), webp (partial, stored as png). An unknown thumb
size or a non-image returns the original unmodified.

**Protected files**: field option `protected: true`, then access needs
`?token=`, and the collection's **`viewRule`** is evaluated with
`@request.context = "protectedFile"`. If `viewRule` is `""` the file is reachable
with no token at all. Tokens come from `POST /api/files/token` and last 180s.

Storage is `pb_data/storage`, one directory per **collection id**, or S3
(`{enabled, bucket, region, endpoint, accessKey, secret, forcePathStyle}`).

## 7. The rest, briefly

- **Collections API** is superuser-only: CRUD on `/api/collections`, plus
  `truncate`, `PUT /api/collections/import`, `meta/scaffolds`,
  `meta/dry-run-view`, `meta/oauth2-providers`.
- **Backups**: list/create/upload/download/delete/restore, plus a `cron` and
  `cronMaxKeep` setting. Since v0.40.0 they no longer transaction-lock the DB.
- **Logs** live in a separate aux database. `GET /api/logs`, `/{id}`, `/stats`,
  and `DELETE /api/logs` (new in v0.40.0).
- **Settings** sit in `_params`: `smtp`, `backups`, `s3`, `meta`, `rateLimits`,
  `trustedProxy`, `batch`, `logs`, `superuserIPs`. Rate-limit audiences are `""`,
  `@guest`, `@auth`.
- **Cron**: `app.Cron().Add(id, expr, handler)`, standard five fields, each job
  in its own goroutine. System jobs are named `__pb*__`. `GET /api/crons` and
  `POST /api/crons/{jobId}` to run one now.
- **Mailer**: per-collection templates with the placeholders `{APP_NAME}`,
  `{APP_URL}`, `{TOKEN}`, `{OTP}`, `{OTP_ID}`, `{ALERT_INFO}`.
- **SQL API**: `POST /api/sql`, superuser-only, arbitrary statements.
- **84 Go hooks** on `core.App`, in a three-phase naming convention —
  `OnXCreate` (before) → `OnXCreateExecute` (around the write) →
  `OnXAfterCreateSuccess` / `OnXAfterCreateError`. Handlers call `e.Next()`.
- **JS VM**: goja, ES5, `*.pb.js` in `pb_hooks/`. Each handler is serialized and
  runs in an isolated context, so outer-scope variables are invisible inside.
  PLAN.md §12 declines this outright.
- **Migrations**: `m.Register(up, down)`, both taking a transactional `core.App`;
  `migrate create|up|down|collections`. Unapplied migrations run on `serve`.

## Not verified

- Whether the Dashboard pre-populates `created`/`updated` on a new collection.
  The API scaffold for `base` contains only `id`.
- The enum values for `POST /api/settings/test/s3`'s `filesystem` parameter.
- `trustedProxy` sub-fields.
- Exact field lists for `_mfas`, `_otps`, `_externalAuths`, `_authOrigins`.
- The docs' record samples still show `username`, which has not been a built-in
  auth field since v0.23. `identityFields` defaults to `["email"]` alone.
