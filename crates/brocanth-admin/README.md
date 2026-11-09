# brocanth-admin

A Django-shaped backoffice for brocanth. It has four screens per model: a
list with pagination, search and sortable columns; a detail view; one form
that creates and edits; and a delete behind a confirmation. Every screen is
an rsx template that you can wrap or replace. Use it when staff must read
and change rows without a page somebody wrote for each table. Do not use it
as a public interface: it ships locked, and a model shows only the rows its
row policy allows. The crate has no SQL driver and no ORM. It reads rows
through an `AdminData` that the application supplies.
`brocanth-admin-model` is that data source over SeaORM.

## Getting started

```toml
[dependencies]
brocanth-admin = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

Features:

| Feature | Default | What it adds |
|---|---|---|
| `runtime` | on | The screens, forms and policy: everything but `scaffold`. A generated project takes `default-features = false, features = ["runtime"]`. |
| `plugin` | on | `AdminPlugin`, a `RouteSource` for a host that mounts one. With it off, `brocanth` is not in the tree, and `Admin::dispatch` still works behind any server. |
| `scaffold` | off | The `admin` and `admin-model` modules for `brocanth create`. Only the CLI turns it on. |

### In a generated project

```sh
brocanth create my-app --with admin
```

The module writes `src/plugins/admin/mod.rs` (yours to edit) and mounts the
admin at `/admin`. It ships locked: no models, the `DenyAll` policy and an
anonymous principal, so `/admin` answers 403. The generated test asserts
that. Two edits open it: a policy with `AdminConfig::with_policy`, and a
`principal` function that reads your user off the request.

`brocanth add admin` works in an existing project. The module changes
`src/pages/mod.rs`: if you have not edited that file, the command rewrites
it. If you have, it prints the change, writes nothing, and goes through
once you have made the change by hand.

`admin-model` reads the rows over SeaORM. It writes
`src/plugins/admin/models.rs`, one line per entity, and the admin is built
over the `brocanth_model::Orm` the `model` plugin puts on the context. See
`brocanth-admin-model`.

The module declares five settings. See [Tuning the admin](#tuning-the-admin).

## Examples

### An admin over an in-memory source

`Fixture::blog()` is a small in-memory source with users and posts. A real
application passes its own `AdminData`.

```rust
use std::sync::Arc;

use brocanth_admin::policy::{policy_fn, Action, Principal};
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture};

struct Staff;

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .titled("Blog admin")
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        // Who may open which screen. The default is `DenyAll`.
        .with_policy(policy_fn(|principal: &Principal<'_>, request| {
            principal.downcast::<Staff>().is_some() && request.action != Action::Delete
        }))
        // The fixture cannot run SQL, so it cannot apply a row policy.
        // A real application registers one per model with `.resource(..)`.
        .unscoped_rows_unsafe_for_development()
        .model("post", |m| {
            m.list_columns(["id", "title", "author_id", "published"])
                .searchable(["title", "body"])
                .foreign_key("author_id", "user", "id")
        })
        .model("user", |m| {
            m.display(|row| row.get("name").map(ToString::to_string).unwrap_or_default())
        }),
);

let staff = Staff;
let response = brocanth_admin::rt::block_on(admin.dispatch(
    Principal::new(&staff),
    "GET",
    "/admin/post",
    "page=1",
    &Default::default(),
))
.unwrap();

assert_eq!(response.status(), 200);
// The foreign key shows the author's name, not the id.
assert!(response.html().unwrap().contains("Ada Lovelace"));
```

### It ships locked

```rust
use std::sync::Arc;

use brocanth_admin::{Admin, AdminConfig, Fixture, Principal};

let admin = Admin::new(Arc::new(Fixture::blog()), AdminConfig::new().at("/admin"));
let response = brocanth_admin::rt::block_on(admin.dispatch(
    Principal::anonymous(),
    "GET",
    "/admin",
    "",
    &Default::default(),
))
.unwrap();
assert_eq!(response.status(), 403);
```

### Rows go through a row policy

Two policies must both say yes. `with_policy` decides who may open a
screen. `AdminConfig::resource(..)` registers a `brocanth-policy` policy per
model and decides which rows: the list and its count filter in the query,
and a write is guarded by its condition. A model with no resource answers
403. The `rows` module has the worked example, and
[`docs/backoffice.md`](../../docs/backoffice.md) §2a has the table of
screens and refusals.

## Tuning the admin

Use the lowest level that does the job. Each level up costs more to keep in
step with new versions of the admin. The full guide, with Django's
equivalent for each level, is
[`docs/admin-customization.md`](../../docs/admin-customization.md).

| Level | What you change | API |
|---|---|---|
| 1. Settings | titles, "View site" link | `AdminConfig::{titled, headed, index_titled, site_url}` |
| 2. Theme | colours, fonts, radii | `AdminConfig::stylesheet` |
| 3. Assets | CSS and JavaScript files, for every screen or one model | `AdminConfig::{stylesheet, script}`, `ModelAdmin::{stylesheet, script}` |
| 3a. Replace a block | the brand or the user links, keep the layout | `Views::{with_brand, with_userlinks}`, `AdminConfig::user_label` |
| 4. Wrap a screen | add markup, keep the default | call `Views::default().<screen>` from your view |
| 5. Replace a screen, one model | one model's list, detail, form or delete | `ModelAdmin::{list_view, detail_view, form_view, confirm_delete_view}` |
| 6. Replace a screen, every model | one screen for all models | `Views::with_<screen>` |
| 7. Replace the layout | the whole shell | `Views::with_layout` |
| 8. Eject a screen | a copy of a shipped screen, as your own file | `brocanth admin eject <screen>`, then `brocanth admin diff` |
| Extra page | a page of your own with the admin's chrome | `AdminConfig::page`, `panel::Panel` |

### Settings: titles, header, index title, site URL

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Fixture, Principal};
use brocanth_test::Html;

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .titled("Acme")                       // the end of every <title>
        .headed("Acme backoffice")            // the top of the sidebar
        .index_titled("Site administration")  // the index's <h1>
        .site_url("/")                        // a "View site" link
        .at("/admin")
        .with_policy(AllowAllUnsafeForDevelopment),
);

let page = brocanth_admin::rt::block_on(admin.index(Principal::anonymous())).unwrap();
let page = Html::new(page.html().unwrap());
assert_eq!(page.tags("h1")[0].text().trim(), "Site administration");
assert_eq!(page.link_to("/admin").unwrap().text().trim(), "Acme backoffice");
assert_eq!(page.link_to("/").unwrap().text().trim(), "View site");
```

`headed` and `index_titled` fall back to the title. With no `site_url`
there is no "View site" link.

#### In a generated project, without code

`brocanth create --with admin` declares these as settings. Each one is a
key in `[admin]` (with the `config` module), an environment variable and a
flag. [`docs/module-flags.md`](../../docs/module-flags.md) has the rules.

| Key | Flag | Variable | `AdminConfig` |
|---|---|---|---|
| `title` | `--admin.title` | `$<APP>_ADMIN__TITLE` | `titled` |
| `header` | `--admin.header` | `$<APP>_ADMIN__HEADER` | `headed` |
| `index_title` | `--admin.index_title` | `$<APP>_ADMIN__INDEX_TITLE` | `index_titled` |
| `site_url` | `--admin.site_url` | `$<APP>_ADMIN__SITE_URL` | `site_url` |
| `stylesheet` | `--admin.stylesheet` | `$<APP>_ADMIN__STYLESHEET` | `stylesheet`, once |

```toml
# config/development.toml
[admin]
title = "Acme admin"
header = "Acme backoffice"
site_url = "/"
stylesheet = "/admin-theme.css"
```

```sh
# With `commands`, the server is the `serve` subcommand.
MY_APP_ADMIN__TITLE="Acme admin" cargo run -- serve
cargo run -- serve --admin.site_url=/   # flags: on without `config`

# Without `commands`, the binary only serves.
cargo run -- --admin.site_url=/
```

- Precedence, lowest first: `Settings::default()`, the config files, the
  environment, a flag.
- With `config`, the binary refuses a flag until `flags_with_config: true`
  in `src/modules/brocanth.rs`, or `flags: Flags::On` in
  `src/modules/admin.rs`. `rename` in `src/modules/admin.rs` renames a flag.
- `mount_at` in `src/modules/admin.rs` is where the backoffice answers,
  `/admin` by default. A tab of another module, such as analytics, is in
  the generated `plugins::admin_panels`.
- All keys except `title` are optional. An empty value, such as
  `--admin.site_url=`, means none, whatever a lower layer set.
- `my-app settings` shows which layer set each value.

The generated `src/plugins/admin/mod.rs` registers the `AdminSettings`
plugin, which claims `[admin]`. Its `mount` function reads the values and
calls the builder methods above. A second stylesheet, a model asset or a
screen is code in that file, as in the levels below.

### Stylesheets and the theme

The shipped stylesheet reads Tailwind tokens as CSS custom properties. A
theme is a short stylesheet that sets them:

```css
/* public/admin-theme.css */
:root {
  --color-accent: #0f766e;
  --color-accent-dark: #115e59;
  --font-sans: "Inter", system-ui, sans-serif;
}
```

These token names are stable API: `--color-ink`, `--color-ink-soft`,
`--color-ink-faint`, `--color-accent`, `--color-accent-dark`,
`--color-line`, `--color-stage`, `--color-danger`, `--font-sans`,
`--font-mono`, `--radius-card`, `--radius-control`. The class names inside
the shipped templates are not stable. Do not target them.

`AdminConfig::stylesheet` links a sheet on every screen.
`ModelAdmin::stylesheet` links a sheet on one model's list, detail, form
and delete screens only. The admin's sheets come first, then the model's.

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal};
use brocanth_test::Html;

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .stylesheet("/admin-theme.css")
        .model("post", |m| m.stylesheet("/admin/posts.css"))
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment),
);

let posts = brocanth_admin::rt::block_on(admin.list(
    Principal::anonymous(),
    "post",
    ListQuery::new(),
))
.unwrap();
let sheets: Vec<String> = Html::new(posts.html().unwrap())
    .tags("link")
    .iter()
    .filter(|l| l.attr("rel") == Some("stylesheet"))
    .filter_map(|l| l.attr("href").map(str::to_string))
    .collect();
assert_eq!(sheets, ["/admin-theme.css", "/admin/posts.css"]);
```

The layout links your sheets after its own `<style>`, and Tailwind puts the
tokens in a layer, so your `:root` rule wins. The admin mounts no asset
route: the application serves the file. A same-origin file passes the
default `Content-Security-Policy`.

### Scripts

`AdminConfig::script` loads a file on every screen. `ModelAdmin::script`
loads one on a model's screens only: Django's `Media.js`. Each is
`<script src="…" defer>` in the `<head>`, the admin's first.

A script is a file and never inline code. A same-origin file passes the
default `script-src 'self'`, so the Content-Security-Policy needs no hash
and no nonce. The admin renders with no request in reach, so a nonce is
not possible; a hash per model script is one more value to keep in step.

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal};

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .model("post", |m| m.script("/admin/posts.js"))
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment),
);
let list = |model| {
    brocanth_admin::rt::block_on(admin.list(Principal::anonymous(), model, ListQuery::new()))
        .unwrap()
        .html()
        .unwrap()
        .to_string()
};
assert!(list("post").contains(r#"<script src="/admin/posts.js" defer></script>"#));
assert!(!list("user").contains("/admin/posts.js"));
```

dp-swap replaces `<main>` and keeps the `<head>`. A model's script runs
when a page of that model loads in full, not after a swap into it. Code
that must run after each swap goes in `AdminConfig::script` and listens
for `dp:afterSwap`.

### Replace a block: the brand and the user links

Django overrides `{% block branding %}` and `{% block userlinks %}`
without copying `base.html`. Here they are two typed views,
`Views::brand` and `Views::userlinks`, each a `View<Chrome>`. The layout
receives them rendered, in `LayoutCtx::brand` and `LayoutCtx::userlinks`.

`AdminConfig::user_label` names who is signed in. The admin has no user
type of its own, so the application turns its principal into a name. The
shipped user links show it, with the "View site" link.

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::view::Chrome;
use brocanth_admin::{Admin, AdminConfig, Fixture, Principal};
use brocanth_test::Html;

struct User {
    email: String,
}

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .titled("Acme")
        .user_label(|who| who.downcast::<User>().map(|u| u.email.clone()))
        .map_views(|views| {
            views
                .with_brand(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
                    w.raw(r#"<a href="/admin"><img src="/logo.svg" alt=""> "#)?;
                    w.text(&ctx.header)?;
                    w.raw("</a>")
                })
                .with_userlinks(|ctx: &Chrome, w: &mut rsx::Writer<'_>| {
                    w.text(ctx.user.as_deref().unwrap_or("Nobody"))?;
                    w.raw(r#" <a href="/logout">Log out</a>"#)
                })
        })
        .at("/admin")
        .with_policy(AllowAllUnsafeForDevelopment),
);

let ada = User { email: "ada@example.com".into() };
let page = brocanth_admin::rt::block_on(admin.index(Principal::new(&ada))).unwrap();
let page = Html::new(page.html().unwrap());
assert_eq!(page.tags("img").len(), 1);
assert!(page.has_link_to("/logout"));
assert!(page.text().contains("ada@example.com"));
```

Two named blocks and not a map from a name to a view: a misspelt name in
a map shows the default and says nothing. A field on `Views` is checked
by the compiler.

### Replace a screen for one model

A screen is a `View` over a typed context. An rsx component that takes the
context is the usual form:

```rsx
// src/views/admin.rsx
use brocanth_admin::view::ListCtx;

pub fn PostList(ctx: &ListCtx) -> Element {
    <section data-model={&ctx.model.key}>
        <h1>Our posts</h1>
        <ol>
            for row in &ctx.rows {
                <li><a href={&row.href}>{&row.label}</a></li>
            }
        </ol>
    </section>
}
```

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::view::ListCtx;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal};
use rsx::Template;

// In an application: `mod views { rsx::load_dir!("src/views"); }`.
# mod views { pub mod admin { rsx::load!("tests/views/custom.rsx"); } }

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .model("post", |m| {
            m.list_view(|ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
                views::admin::PostList { ctx }.render(w)
            })
        })
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment),
);

let list = |model| {
    brocanth_admin::rt::block_on(admin.list(Principal::anonymous(), model, ListQuery::new()))
        .unwrap()
        .html()
        .unwrap()
        .to_string()
};
assert!(list("post").contains("Our posts"));
assert!(!list("user").contains("Our posts"), "other models keep the default");
```

The four per-model methods are `list_view`, `detail_view`, `form_view` and
`confirm_delete_view`. `form_view` serves create and edit;
`FormCtx::is_new` tells them apart. To replace a screen for every model,
use `AdminConfig::with_views(Views::default().with_list(..))`. A per-model
view wins over it.

A replaced screen keeps the admin's checks. The layout still wraps it, a
replaced form still posts to `ctx.action`, and the admin still checks the
policy, the row rules and the CSRF field.

### Wrap a shipped screen

`Views::default()` holds the shipped screens as values. Keep one and call it
from your view. This is Django's `{{ block.super }}`. A wrapped screen gets
the admin's fixes when you upgrade.

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::view::ListCtx;
use brocanth_admin::{Admin, AdminConfig, Discovery, Fixture, ListQuery, Principal, Views};

let shipped = Views::default().list;
let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .model("post", move |m| {
            m.list_view(move |ctx: &ListCtx, w: &mut rsx::Writer<'_>| {
                w.raw("<p role=\"note\">Edits here reach customers.</p>")?;
                shipped.render(ctx, w)
            })
        })
        .at("/admin")
        .discovering(Discovery::new().under("blog::models"))
        .unscoped_rows_unsafe_for_development()
        .with_policy(AllowAllUnsafeForDevelopment),
);

let page = brocanth_admin::rt::block_on(admin.list(
    Principal::anonymous(),
    "post",
    ListQuery::new(),
))
.unwrap();
let html = page.html().unwrap();
assert!(html.contains("Edits here reach customers."));
assert!(html.contains("<table"), "and the shipped table is still there");
```

The unit is a screen. Inside the layout, the brand and the user links
are smaller units: see "Replace a block" above.

### Replace the layout

`Views::with_layout` takes a `View<LayoutCtx>`. `LayoutCtx` carries the
`Chrome` (title, header, site URL, navigation, stylesheets, scripts, the
signed-in user), the heading, the screen's HTML in `body`, and the two
blocks rendered in `brand` and `userlinks`.

```rust
use std::sync::Arc;

use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::view::LayoutCtx;
use brocanth_admin::{Admin, AdminConfig, Fixture, Principal, Views};
use rsx::Template;

# mod views { pub mod admin { rsx::load!("tests/views/custom.rsx"); } }
// `views::admin::Shell` writes <!DOCTYPE html>, a <title> from
// `ctx.heading`, and `ctx.body` inside <body class="our-shell">.
let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .with_views(Views::default().with_layout(
            |ctx: &LayoutCtx, w: &mut rsx::Writer<'_>| views::admin::Shell { ctx }.render(w),
        ))
        .at("/admin")
        .with_policy(AllowAllUnsafeForDevelopment),
);

let page = brocanth_admin::rt::block_on(admin.index(Principal::anonymous())).unwrap();
assert!(page.html().unwrap().contains("our-shell"));
```

Your layout must write `ctx.chrome.stylesheets` and `ctx.chrome.scripts`
itself, or the theme and the model assets stop working. Keep `dp-layout="admin"` around the body if you
want dp-swap navigation.

### Eject a screen

Copying a shipped screen and editing it is often faster than writing one
from an empty file. In a generated project with `admin`:

```sh
brocanth admin eject list                  # every model's list
brocanth admin eject list --model invoice  # the invoice list only
brocanth admin eject layout                # the shell around every screen
```

The screens are `layout`, `index`, `list`, `detail`, `form`, `delete` and
`error`, and the two blocks `brand` and `userlinks`. Only `list`,
`detail`, `form` and `delete` take `--model`.

With the `commands` module, the project binary has the same command,
`cargo run -- admin eject list`. It hands the arguments to the installed
`brocanth` CLI, so the application never links the eject code.

The command writes the shipped template to
`src/plugins/admin/screens/<screen>.rsx` (or
`screens/models/<model>/<screen>.rsx`). That file is yours. It also
writes `src/plugins/admin/ejected.rs`, which installs every copy, and
calls `ejected::apply(config)` in the generated `mount`. The copy renders
the same HTML as the shipped screen until you edit it. The
[CLI README](../brocanth-cli/README.md#brocanth-admin) lists what the
command refuses.

An ejected screen does not get the admin's fixes when you upgrade. A
wrapped screen (level 4) does. After an upgrade, `brocanth admin diff`
prints a unified diff from the shipped screen of the installed version to
each copy:

```sh
brocanth admin diff                       # every ejected copy
brocanth admin diff list --model invoice  # one copy
```

`cargo metadata --offline` finds the installed `brocanth-admin`. When it
cannot, the command compares with the screens the CLI carries and warns.
The header line of the copy names the version it came from. When that is
not the installed version, the diff holds your edits and the release's
changes together, and the command says so.

A copy names the crate through this public API:

| Item | What it is |
|---|---|
| `brocanth_admin::parts` | The class strings the shipped screens use: `heading()`, `card()`, `button()`, `control()` and others. |
| `brocanth_admin::STYLESHEET` | The shipped stylesheet. The layout inlines it in a `<style>`. |
| `brocanth_admin::INLINE_SCRIPT` | The layout's own inline script. |
| `brocanth_admin::MUTATE_SCRIPT` | dp-swap's mutating verbs, the layout's second script. |
| `AdminConfig::map_views` | Changes the screens already installed. `with_views` replaces the whole set. |

`ejected.rs` installs a copy for every model through `map_views`, so a
`with_views` call of your own and an ejected screen both stay:

```rust
use brocanth_admin::view::ErrorCtx;
use brocanth_admin::{AdminConfig, Views};

let config = AdminConfig::new()
    .with_views(Views::default())
    .map_views(|views| {
        views.with_error(|ctx: &ErrorCtx, w: &mut brocanth_admin::rsx::Writer<'_>| {
            w.text(&ctx.message)
        })
    });
# let _ = config;

// The class strings exist for a copy to use. Their values follow the
// stylesheet and are not stable; the names are.
assert!(!brocanth_admin::parts::heading().is_empty());
assert!(!brocanth_admin::STYLESHEET.is_empty());
```

A class that you add to a copy and that `STYLESHEET` does not contain has
no effect. Put its rule in a stylesheet of your own (level 2).

### Add a page

Django's `AdminSite.get_urls()` adds a view with the admin's chrome. Here
that is a `Panel`, the same seam other crates use for their tabs.
`AdminConfig::page` builds one from a closure: the page gets the layout,
a tab after the models, the policy (`Action::ViewPanel` to read,
`Action::ActOnPanel` to post) and every path under its key.

```rust
use std::sync::Arc;

use brocanth_admin::panel::{Page, PanelPage};
use brocanth_admin::policy::AllowAllUnsafeForDevelopment;
use brocanth_admin::{Admin, AdminConfig, Fixture, Principal, Submission};
use brocanth_test::Html;

let admin = Admin::new(
    Arc::new(Fixture::blog()),
    AdminConfig::new()
        .page("reports", "Reports", |request| {
            Ok(match request.path {
                "" => Some(PanelPage::body("Reports", "<p>All quiet.</p>")),
                _ => None, // the admin's own 404
            })
        })
        // No tab: a link from your own screen reaches it.
        .panel(Page::new("wizard", "Wizard", |_| {
            Ok(Some(PanelPage::body("Wizard", "<p>Step one</p>")))
        }).hidden())
        .at("/admin")
        .with_policy(AllowAllUnsafeForDevelopment),
);

let get = |path| {
    brocanth_admin::rt::block_on(admin.dispatch(
        Principal::anonymous(), "GET", path, "", &Submission::new(),
    ))
    .unwrap()
};
let page = Html::new(get("/admin/reports").html().unwrap());
assert!(page.text().contains("All quiet."));
assert_eq!(page.link_to("/admin/reports").unwrap().attr("aria-current"), Some("page"));
assert!(!page.has_link_to("/admin/wizard"));
assert_eq!(get("/admin/wizard").status(), 200);
assert_eq!(get("/admin/reports/nope").status(), 404);
```

The closure is synchronous. A page that awaits a database read
implements `panel::Panel`, whose `render` is async.

### What is stable

- The context types in `brocanth_admin::view` and their field names.
- `View`, `Views` and its `with_*` methods (`with_brand` and
  `with_userlinks` too), `AdminConfig::map_views`, and the `ModelAdmin`
  view methods.
- `panel::Panel`, `panel::Page` and `AdminConfig::page`.
- For ejected screens: the function names in `brocanth_admin::parts`, and
  the constants `STYLESHEET`, `INLINE_SCRIPT` and `MUTATE_SCRIPT`. Their
  values are not stable.
- The CSS custom properties listed above.
- `aria-current="page"` on the current navigation item, and
  `dp-layout="admin"` on the main region.

The markup and the class names inside the shipped templates are not stable.
Assert on behaviour and on the context, not on a class name.

## Testing

```sh
cargo test -p brocanth-admin
```

The tests need no database. They run against `Fixture`, the in-memory
source. `tests/customization.rs` covers levels 1 to 5 above, and
`tests/screens.rs` covers levels 6 and 7. `tests/eject.rs` compiles the
copies in `tests/ejected/` as an outside crate does and compares each
screen's HTML with the shipped screen's, byte for byte (level 8).
`tests/panels.rs` covers panels and extra pages. `tests/ui` holds the compile-fail
cases: an `AdminResource` with no policy does not compile.

In an application, test a screen through the router with
`brocanth_test::TestClient`, or call `Admin::index`, `Admin::list` or
`Admin::dispatch` directly and read the page with `brocanth_test::Html`, as
the examples above do. Assert on a heading, a link or a status, not on a
class name. `brocanth-admin-model`'s `tests/policy.rs` drives the admin
through the router with a real database and row policies.

## Links

- [`docs/admin-customization.md`](../../docs/admin-customization.md): every
  level of customisation, and how it maps to Django.
- [`docs/backoffice.md`](../../docs/backoffice.md): what a backoffice
  contains, the row policies (§2a), and the tabs other crates contribute
  through `panel::Panel`.
- [`docs/module-flags.md`](../../docs/module-flags.md): declared settings,
  flags and environment variables.
- `brocanth-admin-model`: the data source over SeaORM.
