# Customizing the admin

How an application changes what `brocanth-admin` looks like, from one
title to a whole screen. Django's admin is the model. This page says what
Django does, what brocanth does instead, and why.

Use the lowest level that does the job. Each level up costs more to keep
in step with new versions of the admin.

| Level | What you change | API | Django |
|---|---|---|---|
| 1. Settings | titles, "View site" link | `AdminConfig::{titled, headed, index_titled, site_url}`, or `[admin]` and `--admin.*` in a generated project | `AdminSite.site_title`, `site_header`, `index_title`, `site_url` |
| 2. Theme | colours, fonts, radii | `AdminConfig::stylesheet` | `extrastyle` block, CSS variables |
| 3. Assets | CSS and JavaScript, for every screen or one model's | `AdminConfig::{stylesheet, script}`, `ModelAdmin::{stylesheet, script}` | `ModelAdmin.Media`, `extrahead` |
| 3a. Replace a block | the brand or the user links, keep the layout | `Views::{with_brand, with_userlinks}`, `AdminConfig::user_label` | `{% block branding %}`, `{% block userlinks %}` |
| 4. Wrap a screen | add markup, keep the default | capture `Views::default().<screen>` and call it | `{{ block.super }}` |
| 5. Replace a screen, one model | one model's list, detail, form or delete | `ModelAdmin::{list_view, detail_view, form_view, confirm_delete_view}` | `admin/<app>/<model>/change_list.html`, `ModelAdmin.change_list_template` |
| 6. Replace a screen, every model | a screen for all models | `Views::with_<screen>` | `admin/change_list.html` in `DIRS` |
| 7. Replace the layout | the whole shell | `Views::with_layout` | `admin/base_site.html` or `admin/base.html` |
| 8. Eject a screen | start from the shipped screen, then edit | `brocanth admin eject <screen> [--model <key>]`, then `brocanth admin diff` after an upgrade | `vendor:publish` (Laravel); copying from `django/contrib/admin/templates` |
| Extra page | a page of your own with the admin's chrome | `AdminConfig::page`, `Panel` | `AdminSite.get_urls()` |
| Page under a model | a page of your own under one model or one record | `ModelAdmin::{page, record_page}` | `ModelAdmin.get_urls()` |
| Inlines | a child model's rows on the parent's form | `ModelAdmin::inline(Inline)` | `InlineModelAdmin`, `TabularInline` |

The tests for levels 1 to 5 are in
`crates/brocanth-admin/tests/customization.rs`. Levels 6 and 7 are in
`tests/screens.rs`. Level 8 is in `tests/eject.rs` and in
`crates/brocanth-cli/src/commands/admin.rs`. Extra pages are in
`tests/panels.rs`.

## How Django does it

Django finds a template by name. It asks each loader in order and takes the
first file that exists:

1. The directories in `TEMPLATES["DIRS"]`: the project's own templates.
2. With `APP_DIRS = True`, the `templates/` directory of each app in
   `INSTALLED_APPS` order. `django.contrib.admin` is one of these apps.

So `templates/admin/change_list.html` in the project shadows the admin's
own file. That is the whole override mechanism, and it has four parts:

- **Per model and per app.** The admin asks for a list of names, most
  specific first: `admin/<app>/<model>/change_list.html`, then
  `admin/<app>/change_list.html`, then `admin/change_list.html`. Only some
  templates take part: `change_form.html`, `change_list.html`,
  `delete_confirmation.html`, `object_history.html` and a few partials
  such as `submit_line.html` and `pagination.html`. `app_index.html` is per
  app only. `base_site.html` is global only.
- **Per `ModelAdmin`.** `change_list_template`, `change_form_template`,
  `add_form_template`, `delete_confirmation_template` and
  `object_history_template` name a template directly, without the search.
- **Blocks.** `admin/base_site.html` extends `admin/base.html` and is empty
  on purpose, so a project overrides it and fills only the blocks it needs:
  `title`, `branding`, `extrastyle`, `extrahead`, `userlinks`,
  `nav-global`, `content`, `footer`. `{{ block.super }}` renders the
  parent's block inside the override.
- **Assets and theme.** `class Media: css = {"all": [...]}; js = [...]` on a
  `ModelAdmin` adds files to that model's pages. Since Django 3.2 the
  stylesheet reads CSS variables (`--primary`, `--body-bg`, ...), so a
  theme is an `extrastyle` block with a `:root { ... }` rule.

`AdminSite` holds the settings: `site_header`, `site_title`, `index_title`,
`site_url`, and `get_urls()` for extra pages.

Rails engines and Laravel packages do the same by path. A Rails app file at
`app/views/<engine>/...` shadows the engine's view.
`php artisan vendor:publish --tag=<package>-views` copies a package's views
into `resources/views/vendor/<package>/`, where they win.

## Why brocanth does not search by name

A brocanth template is not a file found at run time. `rsx::load_dir!`
compiles every `.rsx` file into Rust at build time, and a screen is a
function that takes a typed context. Three consequences:

- **There is no loader to put first.** A lookup by name would need a
  registry of strings, and a misspelt name would fail silently and show the
  default. Here an override is a value you pass. If it does not compile,
  the build fails.
- **The context is a type.** Django passes a dictionary, and an override
  that reads a key the view stopped sending renders an empty string. Here a
  renamed field is a compile error in your override.
- **Per-model resolution is a field, not a path.** `ModelAdmin::list_view`
  holds the view for one model. The admin asks the model first, then
  `Views`, then the shipped default. That is Django's
  `admin/<app>/<model>/` → `admin/` order, in three lines of code you can
  read in `admin.rs`.

There is no "app" level. A brocanth model has a key and a module path, not
an app label, and no screen groups models by app yet.

## Level 1: settings

```rust
AdminConfig::new()
    .titled("Acme")                      // end of every <title>
    .headed("Acme backoffice")           // top of the sidebar
    .index_titled("Site administration") // the index's <h1>
    .site_url("/")                       // a "View site" link
```

`headed` and `index_titled` fall back to the title. With no `site_url`
there is no link. Django draws "View site" to `/` by default; an admin
here is often mounted beside an API with no front page, so the default is
no link.

### In a generated project, without code

`brocanth create --with admin` declares these as settings, with the theme
of level 2. Each one is therefore a key in `[admin]` (with `config`), an
environment variable and a flag, like every declared setting. See
`docs/module-flags.md`.

| Key | Flag | Variable | `AdminConfig` |
|---|---|---|---|
| `title` | `--admin.title` | `$<APP>_ADMIN__TITLE` | `titled` |
| `header` | `--admin.header` | `$<APP>_ADMIN__HEADER` | `headed` |
| `index_title` | `--admin.index_title` | `$<APP>_ADMIN__INDEX_TITLE` | `index_titled` |
| `site_url` | `--admin.site_url` | `$<APP>_ADMIN__SITE_URL` | `site_url` |
| `stylesheet` | `--admin.stylesheet` | `$<APP>_ADMIN__STYLESHEET` | `stylesheet`, once |

```toml
[admin]
title = "Acme admin"
header = "Acme backoffice"
site_url = "/"
stylesheet = "/admin-theme.css"
```

All but `title` are optional: an empty value, such as `--admin.site_url=`,
means none, whatever a lower layer said. `my-app settings` shows which
layer set each one.

The generated `src/plugins/admin/mod.rs` registers a small plugin,
`AdminSettings`, that claims `[admin]` and keeps the values. `mount` reads
them when the route table is built and calls the builder methods above. A
second stylesheet, a per-model asset or a screen is code in that file, as
in the levels below.

## Level 2: a theme

The shipped stylesheet is built from Tailwind tokens. Tailwind writes each
token as a CSS custom property in `:root`, and every utility reads it with
`var(...)`. So a theme is a short stylesheet the application serves:

```css
/* public/admin-theme.css */
:root {
  --color-accent: #0f766e;
  --color-accent-dark: #115e59;
  --font-sans: "Inter", system-ui, sans-serif;
}
```

```rust
AdminConfig::new().stylesheet("/admin-theme.css")
```

The tokens are in `crates/brocanth-admin/style/admin.css`, under
`@theme`: `--color-ink`, `--color-ink-soft`, `--color-ink-faint`,
`--color-accent`, `--color-accent-dark`, `--color-line`, `--color-stage`,
`--color-danger`, `--font-sans`, `--font-mono`, `--radius-card`,
`--radius-control`. **These names are stable API.** Renaming one is a
breaking change.

The layout links your sheets after its own `<style>`. Tailwind puts the
tokens in `@layer theme`, and a rule outside a layer wins over a rule in a
layer, so your `:root` wins whatever the order.

The admin mounts no asset route. The `href` points at a file the
application serves: the page router's public files, a CDN, anything. A
same-origin file passes the default `Content-Security-Policy`, whose
`style-src` is `'self' 'unsafe-inline'`.

## Level 3: CSS and JavaScript

```rust
AdminConfig::new()
    .script("/admin/everywhere.js")                  // every screen
    .model("invoice", |m| {
        m.stylesheet("/admin/invoices.css")          // Media.css
            .script("/admin/invoices.js")            // Media.js
    })
```

A model's files are on that model's list, detail, form and delete
screens, after the admin's own. The layout writes each stylesheet as a
`<link>` and each script as `<script src="…" defer>` in the `<head>`.

### The Content-Security-Policy decision

**A script is a file the application serves. The admin takes no inline
code for a model.** Three choices were open:

| Choice | Why not, or why |
|---|---|
| A nonce | A fresh value per response, on the header and on each `<script>`. The admin renders a page with no request in reach (`Admin::dispatch` takes a method, a path and form pairs), so it cannot read the nonce. A page with a nonce also differs per visitor, which a shared cache must not store. `brocanth-middleware`'s security headers chose hashes for the same reason. |
| A hash of inline code | Works with `Csp::inline_script(text)`, as `INLINE_SCRIPT` does. But each model script would be one more hash for the application to keep in step with the code, and a file does the same job. |
| **A file** | A same-origin file passes the default `script-src 'self'`. No hash, no nonce, no change to the policy. The browser caches it. |

A file on another origin needs that origin in `script-src`:

```rust
SecurityHeadersConfig::default()
    .csp(Csp::default().allow("script-src", ["https://cdn.example.com"]))
```

The test `a_model_script_is_a_deferred_file_and_never_inline_code` holds
the admin to this: a model script adds no inline `<script>`.

### Scripts and dp-swap

dp-swap replaces the `<main dp-layout="admin">` region. Every admin link
is a `dp-boost` link, so every swap is a navigation. The response is the
whole document, and on a navigation dp-swap merges its `<head>` into the
live one:

- A stylesheet the live page does not link yet is added before the swap,
  and the swap waits until it has loaded or failed, for 3 seconds at
  most (dp-swap's `styleTimeout`). A sheet slower than that arrives after
  the screen.
- A script the live page does not load yet is added after the swap, so
  it finds its markup. Its top level runs once, the first time a screen
  names it. `DOMContentLoaded` does not fire again.
- When every new script has loaded or failed, dp-swap fires
  `dp:headLoaded` on `document`. It fires after each navigation, also
  when no script was new.
- Each file is added once. Inline `<script>` and `<style>` are never
  copied, and a copy carries no `on*` attribute, so the
  Content-Security-Policy decision above holds.
- Only a file of the page's origin is added. A model script or sheet on
  a CDN loads on a full load and not on a swap, unless an every-screen
  script (`AdminConfig::script`) allows that origin:
  `dpswap.configure({ headOrigins: ["https://cdn.example.com"] })`. That
  script runs after dp-swap, because it is deferred.
- Nothing is removed. A model's stylesheet stays after a swap to another
  screen, so write its rules for that model's markup.

So a script from `ModelAdmin::script` starts in one of two places: at
top level, which covers a full load and the first swap into its screen,
and on `dp:headLoaded`, which covers each later swap. Use both when the
script works on the screen's markup, and make the setup safe to run
twice on the same element:

```js
// /admin/invoices.js
function setup() {
  for (const el of document.querySelectorAll("[data-total]:not([data-ready])")) {
    el.dataset.ready = "";
    // ...
  }
}
setup();
document.addEventListener("dp:headLoaded", setup);
```

Do not wait for `dp:afterSwap` in a merged script: it fires before the
script has loaded, so the script misses the swap that brought it.

**Why the admin sets header mode.** dp-swap's default asks for
`<url>/_fragment.html`, the prerendered fragment of a static site. The
admin serves no such path, so every boosted link was a 404, and dp-swap
fell back to a full load. In header mode dp-swap asks for the page's own
URL, the admin answers with the whole document, and dp-swap swaps the
`dp-layout="admin"` region. The admin does not trim the layout for a
`DP-Partial` request: the whole document is what carries the `<head>`.

The line that sets header mode is appended to `INLINE_SCRIPT`, not put in
a script of its own, so the one hash a policy holds for it covers it.

**Why in dp-swap.** The fault is in the client: it threw the response's
`<head>` away. The fix is in dp-swap (`src/head.ts`, commit `54049d1` of
`brocanth/htmx`, vendored in `crates/brocanth-admin/vendor/` and
`crates/brocanth-cli/vendor/`), so every server-rendered page that uses
dp-swap gets it, not only the admin. The tests are in dp-swap
(`test/head.test.ts`). `tests/customization.rs` holds the admin to its
half: header mode, the merge and `dp:headLoaded` in the vendored build,
a response that starts with its doctype (dp-swap tells a whole document
by its start), and the model's files in the `<head>` of the page a swap
gets.

## Level 3a: replace a block

Django overrides `{% block branding %}` or `{% block userlinks %}`
without copying `base.html`. Here the two are typed views of their own:

| Block | `Views` field | Default | Django |
|---|---|---|---|
| Brand | `brand: View<Chrome>` | the header, linked to the index | `branding` |
| User links | `userlinks: View<Chrome>` | "Signed in as …" and "View site" | `userlinks` |

```rust
AdminConfig::new()
    // Who is signed in: the principal is yours, so you name it.
    .user_label(|who| who.downcast::<User>().map(|u| u.email.clone()))
    .map_views(|views| {
        views
            .with_brand(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
                views::admin::Brand { ctx }.render(w)  // a logo and ctx.header
            })
            .with_userlinks(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
                views::admin::UserLinks { ctx }.render(w)  // "Log out"
            })
    })
```

The admin renders each block, then gives the HTML to the layout in
`LayoutCtx::brand` and `LayoutCtx::userlinks`. A layout of your own
(level 7) writes them where it wants, or ignores them.

**Why two named blocks and not a general block mechanism.** A general one
is a map from a block name to a view. A misspelt name would then fail
silently and show the default, which is the Django failure this design
removes (see "Why brocanth does not search by name"). A field on `Views`
is checked by the compiler. The two blocks are the two Django
applications override most. The next block follows the same pattern: a
field on `Views`, a template in `src/templates/`, a field on `LayoutCtx`.

**Why the admin draws no "Log out".** It has no authentication scheme
(PLAN.md §8). The application that has one replaces `userlinks`.

Both blocks can be ejected: `brocanth admin eject brand`.

## Level 4: wrap a shipped screen

`Views::default()` holds the shipped screens as values. Keep one and call
it from your override. This is `{{ block.super }}`:

```rust
let shipped = Views::default().list;
.model("invoice", |m| {
    m.list_view(move |ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
        w.raw("<p role=\"note\">Changes here reach customers.</p>")?;
        shipped.render(ctx, w)
    })
})
```

The unit is a screen. Inside the layout, two blocks are smaller units:
level 3a.

## Level 5 and 6: replace a screen

Write a component that takes the screen's context:

```rust
// src/views/admin.rsx
use brocanth_admin::view::ListCtx;

pub fn InvoiceList(ctx: &ListCtx) -> Element {
    <h1>{&ctx.model.plural}</h1>
    <ol>
        for row in &ctx.rows {
            <li><a href={&row.href}>{&row.label}</a></li>
        }
    </ol>
}
```

Give it to one model, or to every model:

```rust
mod views { rsx::load_dir!("src/views"); }

// One model. Wins over the line below.
.model("invoice", |m| m.list_view(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
    views::admin::InvoiceList { ctx }.render(w)
}))
// Every model.
.with_views(Views::default().with_list(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
    views::admin::InvoiceList { ctx }.render(w)
}))
```

The layout still wraps a replaced screen. The policy, the row rules and
the CSRF field do not depend on the view: a replaced form still posts to
`ctx.action`, and the admin still checks the submission.

Per model: `list_view`, `detail_view`, `form_view`,
`confirm_delete_view`. The form serves create and edit, so `form_view` is
Django's `add_form_template` and `change_form_template` together;
`FormCtx::is_new` tells them apart. The index and the error page are not
per model, because they do not show one model.

## Level 7: replace the layout

`Views::with_layout` takes a `View<LayoutCtx>`. `LayoutCtx` carries the
`Chrome` (title, header, site URL, nav, stylesheets) and the screen's HTML
in `body`. A layout of your own must write `ctx.chrome.stylesheets` itself
if it wants levels 2 and 3 to keep working, and must keep
`dp-layout="admin"` around the body if it wants dp-swap navigation.

## Level 8: eject a screen

Copying a default and editing it is often faster than writing one from an
empty file. That is Laravel's `vendor:publish`. In a generated project:

```
brocanth admin eject list                  # every model's list
brocanth admin eject list --model invoice  # the invoice list only
brocanth admin eject layout                # the shell around every screen
```

**From the project's own command line.** With the `commands` module, the
project binary has the same command: `cargo run -- admin eject list` (or
`my-app admin eject list`). It runs `brocanth admin eject` with the same
arguments in the project root (the nearest directory with
`brocanth.toml`) and exits with its status. The work stays in the
`brocanth` CLI, so the application never links the code that writes
source. Without the CLI on `PATH`, it stops with a `help:` line that says
how to install it. The command is `src/commands/admin.rs`, and it is
yours.

Screens: `layout`, `index`, `list`, `detail`, `form`, `delete`, `error`,
and the two blocks of the layout, `brand` and `userlinks`. `list`,
`detail`, `form` and `delete` take `--model`; the others show no one
model.

The command writes the shipped template into the project and installs it.
The copy renders the same HTML as the shipped screen until you edit it.
`tests/eject.rs` holds the crate to that, byte for byte, on every screen.

| Path | Owner |
|---|---|
| `src/plugins/admin/screens/list.rsx` | you, from the moment it is written |
| `src/plugins/admin/screens/models/invoice/list.rsx` | you |
| `src/plugins/admin/ejected.rs` | the command: written again on every eject |

It also makes three edits, each once and only when missing:

- `rsx` in `[dependencies]`, because the copy is a template the project
  compiles;
- `mod ejected;` in `src/plugins/admin/mod.rs`;
- `let config = ejected::apply(config);` in `mount`, before
  `Admin::new`. If you rewrote `mount` and that line is gone, the command
  writes the copy, warns, and tells you where the line goes.

`ejected.rs` installs each copy through the same API as levels 5 to 7:
`Views::with_<screen>` through `AdminConfig::map_views`, or
`ModelAdmin::<screen>_view` through `AdminConfig::model`. `map_views`
changes the screens already installed instead of replacing the set, so an
ejected screen and a `with_views` call of your own both survive.

`ejected.rs` is rebuilt from the `.rsx` files on disk. To drop an ejected
screen, delete its file and run any eject again, or remove its lines from
`ejected.rs` by hand.

**What it refuses.** It never overwrites a copy that exists: edit that
file, or delete it to start again from the shipped screen. It refuses
`--model` on `layout`, `index`, `error`, `brand` and `userlinks`, and a
model key that is not a
Rust module name (`blog-post`), because `rsx` turns each directory into a
module.

**What changes in the copy.** The paths only. The shipped template names
the crate as `crate::`; the copy names its public API: `brocanth_admin::parts`,
`brocanth_admin::view`, `brocanth_admin::form`, `brocanth_admin::paging`,
and the layout's inlined assets as `brocanth_admin::STYLESHEET`,
`INLINE_SCRIPT` and `MUTATE_SCRIPT`. A header line names the version it
came from.

**Classes.** The shipped stylesheet holds only the utility classes the
shipped screens use. A class you add to a copy that the sheet does not
contain does nothing. Put the rules for it in a stylesheet of your own
(level 2).

Ejecting and wrapping are different trades. A wrapped screen (level 4)
gets the admin's fixes for free. An ejected screen does not: after an
upgrade, compare your copy with the new default.

### After an upgrade: `brocanth admin diff`

```
brocanth admin diff                       # every ejected copy
brocanth admin diff list --model invoice  # one copy
brocanth admin diff --shipped ../brocanth/crates/brocanth-admin
```

It prints a unified diff from the shipped screen to your copy, with the
same path rewrite an eject applies, so a fresh copy shows no difference.
With `commands`, the project binary has it too: `cargo run -- admin diff`.

**Which shipped screen.** The one of the `brocanth-admin` the project
installed. `cargo metadata --offline` names its directory, and the
command reads `src/templates/<screen>.rsx` there. It does not fetch: a
project that was never built has nothing on disk, and the command then
compares with the screens the CLI carries, and warns. `--shipped <dir>`
names a directory.

**What the diff means.** The copy's header line names the version it
came from. The command prints that version and the shipped one:

- The same version: the diff is your edits.
- Another version: the diff holds your edits and the release's changes
  together. Read the `-` lines that you did not remove: those are the
  release's.

A three-way diff would separate the two. It needs the template of the
version the copy came from, and nothing keeps that on disk after the
upgrade. A copy with no header line still compares; the command says it
cannot name the version.

The command exits 0 whether the copies differ or not. It reports; it does
not judge an edit.

## Extra pages: `AdminConfig::page`

Django's `AdminSite.get_urls()` adds a view with the admin's chrome. Here
that is a `Panel`, the tab other crates contribute (`docs/backoffice.md`).
There is no second concept, because a panel already has all four parts
an extra page needs:

| Part | How |
|---|---|
| The chrome | the layout wraps `PanelPage::Body` |
| Authorization | `Action::ViewPanel` to read, `Action::ActOnPanel` to post, asked before the page runs |
| A navigation entry | a tab after the models; `Page::hidden` or `Panel::in_nav` removes it |
| Its own paths | everything under `{base}/{key}`; `None` is the admin's 404 |

`AdminConfig::page` is the short form. The closure renders one request:

```rust
AdminConfig::new().page("reports", "Reports", |request| {
    Ok(match request.path {
        "" => Some(PanelPage::body("Reports", render_reports()?)),
        "export" => Some(PanelPage::body("Export", render_export()?)),
        _ => None,
    })
})
```

A page with no tab, reached by a link from your own override:

```rust
.panel(Page::new("wizard", "Wizard", render_wizard).hidden())
```

The closure is synchronous. A page that awaits something, such as a
database read, implements `Panel`, whose `render` is async. A page key
that is also a model key stops `Admin::new`, naming both.

## Pages under one model: `ModelAdmin::page` and `record_page`

Django's `ModelAdmin.get_urls()` adds a view under one model:
`/admin/invoice/overdue`, `/admin/invoice/7/send`. Here a model adds two
kinds of page:

```rust
AdminConfig::new().model("invoice", |m| {
    m.page("overdue", "Overdue", |request, _| {         // {base}/invoice/overdue
        Ok(match request.path {
            "" => Some(PanelPage::body("Overdue invoices", render_overdue()?)),
            _ => None,
        })
    })
    .record_page("send", "Send", |request, invoice| {   // {base}/invoice/7/send
        let invoice = invoice.expect("a record page gets its record");
        Ok(Some(match request.post {
            false => PanelPage::body("Send", render_send_form(invoice)?),
            true => {
                send(invoice)?;
                PanelPage::redirect(request.mount)
            }
        }))
    })
})
```

The closure takes the request a panel takes (`PanelRequest`: the path
below the page, the query, the POST and its form, `can_act`, `mount`)
and the record, and returns a `PanelPage`. `None` is the admin's 404.

| | Under the model | Under a record |
|---|---|---|
| Path | `{base}/{model}/{key}` and below | `{base}/{model}/{record}/{key}` and below |
| Read | `Action::ViewList` on the model, and the list rule is open | `Action::ViewDetail` on the record, and the record is read under the view rule: a refused record is a 404 |
| POST | also `Action::Update` on the model | also `Action::Update` on the record and its update rule |
| Link | a button on the list screen | a button on the detail screen, before the `record_link`s |
| Layout | the admin's, with the model marked in the sidebar | the same |

The tests are in `crates/brocanth-admin/tests/model_pages.rs`.

**Why the list's and the record's policy, and no new `Action`.** A page
under a model shows something about that model, so the people who may
list it may read it; one under a record is about that record, so it
answers as the detail screen does, with the same 404 for a record the
rule refuses. A POST changes something, and `Action::Update` is what an
action over selected rows asks too. A new `Action` variant would break
every policy that matches on the enum, for a distinction no
application has asked for yet. `can_act` tells the page whether to draw
its buttons, as it tells a panel.

**Why not a panel.** A panel is under its own key, has its own tab, and
asks `ViewPanel` about that key. A page under a model belongs to the
model: its URL, its sidebar entry, its policy and its link on the
model's screens. Both take the same request and return the same page, so
a page moves from one to the other without a rewrite.

**What a page key takes.** A page key wins over a record key at the same
place, so a record whose key is `overdue` cannot be read. That is
Django's choice for `add`, and the admin's for `new`. `Admin::new`
refuses a key that the admin's own screens use there (`new` and `actions`
under a model, `edit` and `delete` under a record), a key with a slash,
and two pages with one key, each with a `help:` line.

**What the page writes is its own.** The admin asks the policy and the
update rule before it calls the page, and passes the record it read. It
does not guard the page's own writes with that rule, because it does not
see them. A page that writes the record should go through the same rule
in its own code, or be an action over selected rows
(`ModelAdmin::action`), which the admin guards.

## What is stable API

- The context types in `brocanth_admin::view` and their field names. A
  new field is a minor change; a renamed or removed field is breaking.
- `View`, `Views` and its `with_*` methods (`with_brand` and
  `with_userlinks` too), `AdminConfig::map_views`, and the `ModelAdmin`
  view methods.
- `Panel`, `Page`, `AdminConfig::page`.
- The CSS custom properties listed under level 2.
- For ejected screens: the function names in `brocanth_admin::parts`
  (`heading`, `card`, `button`, `control`, ...), and the constants
  `STYLESHEET`, `INLINE_SCRIPT` and `MUTATE_SCRIPT`. Their *values* are
  not stable: a class string or the inlined sheet changes with the
  stylesheet. A copy keeps compiling across an upgrade; it does not keep
  looking identical.
- `aria-current="page"` on the current nav item, and `dp-layout="admin"`
  on the main region.

Not stable: the markup and the class names inside the shipped templates.
Assert on behaviour and on the context, not on a class name. A theme that
targets `.bg-white` breaks on the next stylesheet build.

## Actions, the journal and record links

- **Actions over selected rows** (`brocanth_admin::actions`): Django's
  `ModelAdmin.actions`. `AdminConfig::action` adds one to every model,
  `ModelAdmin::action` to one, `ModelAdmin::without_action` drops one. A
  confirmation page comes first; the rules of the single-record screens
  hold for every record. The `action` screen ejects like the others
  (`with_confirm_action`). See `docs/modules/admin-actions.md`.
- **The journal** (`AdminConfig::journal`): after each write the admin
  makes, a `Journal` hears who, which record, and the values before and
  after. Django's `LogEntry`, without a table: `brocanth-audit` keeps it.
- **Record links** (`AdminConfig::record_link`): a link on every detail
  screen. Django's History button is one; `brocanth-audit` adds it.

## Inlines: child rows on the parent's form

Django's `InlineModelAdmin`. The posts of a user on the user's create and
edit form: change a post, delete it, or add one, in the same POST as the
user.

```rust
AdminConfig::new().model("user", |m| {
    m.inline(
        Inline::new("post")              // the child model's key
            .fields(["title", "published"]) // default: its form fields
            .extra(1)                    // blank rows for new posts
            .max(50),                    // existing rows shown
    )
})
```

`Inline::new` finds the child column that points at the parent from the
schema. Name it with `Inline::fk("author_id")` when two columns point at
the parent, or when the schema does not report the relation. `Admin::new`
stops, with a `help:` line, when the child model is not shown, when no
column or more than one points back, or when two inlines name one child.

The tests are in `crates/brocanth-admin/tests/inlines.rs` (the fixture)
and `crates/brocanth-admin-model/tests/inlines.rs` (SQLite, and
PostgreSQL when `BROCANTH_TEST_DATABASE_URL` is set).

### How it works

- **One form, one POST.** The rows are a table inside the parent's
  `<form>`. So the one CSRF token the form carries covers them, and the
  form works without JavaScript. A row posts `{model}-{n}-{column}`, its
  key in `{model}-{n}-pk` and its delete box in `{model}-{n}-delete`, as
  Django's formsets do. `n` is the row's place on the form; the key says
  which child the row changes.
- **The children are read in the query.** The admin lists the child model
  with `ListRequest::filter` set to `<fk> = <parent key>`, under the child's
  list rule, in key order. It reads at most `max` rows; with more, the form
  shows the first ones and says so. The POST cannot change the rows it did
  not show.
- **The child's rules, per row.** Each existing row asks the screen policy
  `Action::Update` and `Action::Delete` on that record, and the child's
  row rule for that record (one guarded select when the rule depends on
  the row). A row the rules refuse is drawn read-only, posts no key, and
  has no delete box. A new row needs `Action::Create` and a create rule
  that admits every row, as the child's own create form does. The field
  mask locks the fields it refuses; the column that points at the parent
  must be writable to add a child, because the admin writes it.
- **Everything is checked before anything is written.** The admin
  validates the parent and every row, asks every rule, and admits every
  write with the same guarded select the child's own screens use. A POST
  that changes a row the form drew read-only, or sends a key that is not
  one of this parent's rows, is refused (403, 409) and writes nothing. A
  form error on any row re-renders the whole form with 422: each error is
  beside its field in its row, and every typed value stays.
- **A blank row adds nothing.** A new row that comes back with every field
  empty is skipped. A `<select>` on a row always offers the empty choice,
  so a blank row really posts nothing.

### Decision: no transaction, and a partial save is shown as one

`AdminData` has no transaction, and the admin does not hold the
connection. So the writes are not atomic: the parent first, then each
child row in form order. The admin narrows the window it can control:
every check and every form error comes before the first write. What is
left is a refusal by the data source itself, such as a unique index, on
one child row.

Then the admin stops at that row and answers 422. The row shows the
error; the form says that the record and the rows before the marked one
were saved. The form that comes back is safe to post again: an added row
comes back as an existing child with its key, a deleted row is gone, and
a create form comes back as the new record's edit form. Django wraps the
same writes in `transaction.atomic`. Here that waits for a transaction
on `AdminData`.

Two other choices were open:

| Choice | Why not |
|---|---|
| A `transaction` method on `AdminData` | Every data source must then implement it, and the in-memory fixture and a source over an HTTP API cannot. It is the right next step for SQL sources, behind a default method. |
| Write the children first | A new parent has no key until it is written, and a child needs it. |

### Decision: an exact-match filter on `ListRequest`

The children of one record are a filter, not a search: a search is
`LIKE` on text, and the key is an exact value. So `ListRequest` has a
`filter: Option<Equals>`, which a data source must `AND` into its `WHERE`
clause, beside the scope. The value is typed by the child column's kind,
so an integer column gets an integer bind (PostgreSQL does not compare an
integer with text). The fixture and `brocanth-admin-model` apply it.

The PostgreSQL test found the same fault in `brocanth-admin-model`'s
key lookup: it bound the key from the URL as text, so every edit form
over PostgreSQL answered 500, and SQLite's type affinity hid it.
`single_key` now binds an integer key as an integer.

The alternative was to fold the filter into the row policy's condition.
That makes the policy carry something that is not a policy, and the
fixture applies no condition.

## Open

- **Per-app overrides.** No app grouping exists.
- **Ejecting a panel.** `brocanth admin eject` copies the shipped screens
  and the two blocks. A `Panel` renders its own body in the crate that
  owns it, so there is no shipped panel template to copy.
- **An async page under a model.** `ModelAdmin::page` takes a
  synchronous closure, as `AdminConfig::page` does. A page that awaits a
  database read is a `Panel` today, under its own key.
- **More blocks.** `brand` and `userlinks` exist. `nav-global` and
  `footer` follow the same pattern when an application needs them.
- **dp-swap's `main`.** The vendored build is `f80aeeb` plus the head
  merge and its fixes (`54049d1`). The same change is on branch `feat/merge-head-main` of
  `brocanth/htmx`, on top of its `main`; neither branch is pushed. Moving
  the admin to dp-swap's `main` also brings its meta-element
  configuration (`<meta name="dp-config">`), which could replace the
  appended `configure` line.
- **A three-way diff.** `brocanth admin diff` is two-way. A three-way diff
  needs the template of the version a copy came from.
