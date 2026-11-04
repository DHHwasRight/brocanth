# brocanth-robots

`brocanth-robots` writes a `robots.txt` from a builder and checks it before
anything serves it. It refuses the two lines crawlers silently ignore: a
rule path that does not start with `/`, and a sitemap that is not a full
URL. It has no dependencies.

It also ships the `robots` module. The module serves the file at
`/robots.txt` from settings, and answers `Disallow: /` on development and
staging servers so they are never indexed.

Use it when a site is public. Do not use it to hide private pages: a
robots.txt is a request to polite crawlers, and it lists the paths it
hides. Protect a private page with authentication.

## Getting started

In a generated project:

```text
brocanth create my-site --with robots
brocanth add robots
```

The "A static site" wizard preset ticks it: the static export writes the
file as `robots.txt`, open, because the export is the published site.

By hand:

```toml
[dependencies]
brocanth-robots = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

### Settings

The module declares four settings in `[robots]`, plus groups for named
crawlers. See
`docs/module-flags.md` for flags, environment variables and precedence.

| Key | Default | Flag | Environment |
|---|---|---|---|
| `index` | `true` in code; `false` in `config/development.toml` (`true` with `static`) | `--robots.index` | `$<APP>_ROBOTS__INDEX` |
| `disallow` | `[]`, or `["/admin"]` with `admin` | none (a list) | by index, with `config` |
| `allow` | `[]` | none (a list) | by index, with `config` |
| `sitemap` | none | `--robots.sitemap` | `$<APP>_ROBOTS__SITEMAP` |
| `groups` | none | none (records) | none: file only |

`index = false` writes `User-agent: *` and `Disallow: /`, and nothing else:
no groups, no allow rules, no sitemap.

### Groups for named crawlers

Each `[[robots.groups]]` table is one group, written after the `*` group.
`agents` names the crawlers; `allow` and `disallow` are their rules. A
group is a record, so it lives in the file only and has no flag. The
generated file holds this example, commented out. It asks crawlers that
collect training data to read nothing:

```toml
[[robots.groups]]
agents = ["GPTBot", "CCBot", "Google-Extended", "ClaudeBot"]
disallow = ["/"]
```

robots.txt is a request, not a lock. A crawler may ignore it, and these
product tokens change; check each operator's own documentation. A crawler
that matches a named group follows that group only, not the `*` rules.

## Examples

Every crawler may read everything:

```rust
use brocanth_robots::Robots;

assert_eq!(Robots::new().render().unwrap(), "User-agent: *\nDisallow:\n");
```

Keep crawlers out of one area, open one page inside it, and name the
sitemap:

```rust
use brocanth_robots::Robots;

let text = Robots::new()
    .disallow("/admin")
    .allow("/admin/help")
    .sitemap("https://example.com/sitemap.xml")
    .render()
    .unwrap();
assert_eq!(
    text,
    "User-agent: *\nDisallow: /admin\nAllow: /admin/help\n\nSitemap: https://example.com/sitemap.xml\n"
);
```

A group for named crawlers, written after the `*` group:

```rust
use brocanth_robots::{Group, Robots};

let text = Robots::new()
    .group(Group::new("GPTBot").agent("CCBot").disallow("/"))
    .render()
    .unwrap();
assert!(text.ends_with("User-agent: GPTBot\nUser-agent: CCBot\nDisallow: /\n"));
```

A staging server, and a mistake the builder refuses:

```rust
use brocanth_robots::{Robots, RobotsError};

assert_eq!(Robots::closed().render().unwrap(), "User-agent: *\nDisallow: /\n");

let error = Robots::new().disallow("admin").render().unwrap_err();
assert_eq!(error, RobotsError::Path("admin".to_string()));
assert!(error.to_string().contains("help:"));
```

## Testing

```text
cargo test -p brocanth-robots
cargo test -p brocanth-robots --features scaffold
```

No database or network is needed. In an application, test the route with
`brocanth-test` (see its README): send `GET /robots.txt` and assert on the
body and the `content-type`.

## More

- `docs/robots.md`: the design and what is left for later (sitemap.xml).
- `docs/modules/robots.md`: the module's manifest.
- `docs/static-export.md`: how the export writes `robots.txt`.
