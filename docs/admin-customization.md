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
| 3. Model assets | CSS for one model's screens | `ModelAdmin::stylesheet` | `ModelAdmin.Media` |
| 4. Wrap a screen | add markup, keep the default | capture `Views::default().<screen>` and call it | `{{ block.super }}` |
| 5. Replace a screen, one model | one model's list, detail, form or delete | `ModelAdmin::{list_view, detail_view, form_view, confirm_delete_view}` | `admin/<app>/<model>/change_list.html`, `ModelAdmin.change_list_template` |
| 6. Replace a screen, every model | a screen for all models | `Views::with_<screen>` | `admin/change_list.html` in `DIRS` |
| 7. Replace the layout | the whole shell | `Views::with_layout` | `admin/base_site.html` or `admin/base.html` |
| 8. Eject a screen | start from the shipped screen, then edit | `brocanth admin eject <screen> [--model <key>]` | `vendor:publish` (Laravel); copying from `django/contrib/admin/templates` |

The tests for levels 1 to 5 are in
`crates/brocanth-admin/tests/customization.rs`. Levels 6 and 7 are in
`tests/screens.rs`. Level 8 is in `tests/eject.rs` and in
`crates/brocanth-cli/src/commands/admin.rs`.

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

## Level 3: CSS for one model

```rust
.model("invoice", |m| m.stylesheet("/admin/invoices.css"))
```

The file is linked on that model's list, detail, form and delete screens,
after the admin's own sheets. There is no `js` counterpart yet: the layout
already runs dp-swap, and a script per model needs a CSP decision first
(`INLINE_SCRIPT` is the only inline script the admin allows by hash).

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

The unit is a screen, not a block. The shipped screens have no named
blocks to fill one at a time. See "Open" below.

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

Screens: `layout`, `index`, `list`, `detail`, `form`, `delete`, `error`.
`list`, `detail`, `form` and `delete` take `--model`; the other three show
no one model.

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
`--model` on `layout`, `index` and `error`, and a model key that is not a
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
upgrade you compare your copy with the new default yourself. The header
line in the copy says which version to compare with.

## What is stable API

- The context types in `brocanth_admin::view` and their field names. A
  new field is a minor change; a renamed or removed field is breaking.
- `View`, `Views` and its `with_*` methods, `AdminConfig::map_views`, and
  the `ModelAdmin` view methods.
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

## Open

- **Blocks smaller than a screen.** Django overrides `branding` or
  `userlinks` without copying `base.html`. Here the smallest unit is a
  screen or the layout. A `Views::brand` and `Views::userlinks` pair would
  cover the two most-used Django blocks; it waits for a second application
  that needs it.
- **Per-app overrides.** No app grouping exists.
- **Extra pages on the admin.** Django's `get_urls()`. A `Panel` is the
  closest thing: a tab with the admin's chrome. See `docs/backoffice.md`.
- **Per-model JavaScript.** See level 3.
- **Ejecting a panel.** `brocanth admin eject` copies the seven shipped
  screens. A `Panel` renders its own body in the crate that owns it, so
  there is no shipped panel template to copy.
- **A diff after an upgrade.** The command does not compare a copy with
  the new shipped screen. The header line names the version; the shipped
  templates are in `crates/brocanth-admin/src/templates/`.
