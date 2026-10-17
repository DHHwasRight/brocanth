# nested-cards

[prerender][]'s `nested-cards`, served by brocanth.

The original builds every route several times — once as a document, once for
each number of layout layers a visitor might already hold — and serves the
result as static files. This renders the same answers from the same templates,
at request time, from one binary.

Nothing about the page changed. Three of the four partial depths of
`/delta/card-6/activity` are byte for byte what the static build writes, and
the fourth differs by the 25 bytes of two fingerprinted asset URLs:

| | brocanth | prerender |
|---|---|---|
| document | 2957 | 2932 |
| `DP-Partial: 1` | 2468 | 2468 |
| `DP-Partial: 2` | 1791 | 1791 |
| `DP-Partial: 3` | 425 | 425 |

The original's own twelve Playwright tests pass against this server unaltered.

```sh
cargo run -p nested-cards      # http://127.0.0.1:3000
```

## What the directory says

prerender registers four layouts against path prefixes, in `main`. Here the
directory is the registration:

```
src/pages/layout.rsx                        the shell      covers /
src/pages/index.rs                          -> /alpha
src/pages/[section]/layout.rsx              the section    covers /{section}
src/pages/[section]/index.rs + .rsx         the grid
src/pages/[section]/[card]/layout.rsx       the card       covers /{section}/{card}
src/pages/[section]/[card]/index.rs         -> …/overview
src/pages/[section]/[card]/[tab].rs + .rsx  one panel
```

Four layers, and no file lists them. `html!` reads the chain off the tree
while the crate compiles and expands into a `match` over the depth the request
asked for.

## What a layout is handed

A layout declares what it wants and `html!` fills it:

- `layout`, the URL prefix it covers. Under a parameter this is resolved per
  request, so `src/pages/[section]` marks itself `/alpha`, never `/[section]`.
  A marker that read the same under every section would tell the client that
  two sections share a layer they do not.
- `path`, the request being answered, for the chrome that marks where you are.
- `children`, the layer below.

## Out of band

A partial does not contain the layers the client keeps, so their chrome goes
stale: the sidebar still marks the section you came from. A layout that has
something to correct defines `Oob` beside `Layout`, and the answer carries it.

Only the **deepest kept layer** sends one. A layer that is shared means
everything it scopes is unchanged — if the section layout is kept then the
section did not change, so the sidebar above it cannot be stale.

## The differences from the original, in full

- Assets are fingerprinted and embedded: `/css/site.<hash>.css` rather than
  `/site.css`. That is the 25 bytes.
- The data is a `OnceLock` rather than an `Rc` threaded through closures,
  because handlers are not closures here.
- A layout reads what it needs out of the path rather than being handed a
  record, since it sits above pages that load different things.
- There is no build step and no `dist/`. The binary is the deployment.

[prerender]: https://github.com/DHHwasRight/prerender
