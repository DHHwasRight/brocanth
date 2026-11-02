# brocanth-bundle

JS and CSS bundles, built by `cargo build`. Call it from `build.rs`. It finds
every input file under `assets/` (a basename beginning with `_`) and writes
the published file beside it. Rolldown bundles a JS or TypeScript entry with
everything it imports, `node_modules` included. Lightning CSS inlines the
`@import`s of a CSS entry. `brocanth-assets` then embeds and fingerprints
the outputs like any other file. Use it when a page needs more than one
hand-written script or stylesheet: TypeScript, npm packages, code split
with `import()`. Do not use it for Tailwind: the `rsx-tailwind` module runs
the Tailwind CLI. Do not use it when `assets/` holds only finished files:
`brocanth-assets` serves those as they are.

## Getting started

No `brocanth create` module adds this crate yet. Add it by hand, as a build
dependency:

```toml
[build-dependencies]
brocanth-bundle = { git = "https://github.com/DHHwasRight/brocanth.git" }
```

```rust,no_run
// build.rs
fn main() {
    brocanth_bundle::build().unwrap_or_else(|e| panic!("{e}"));
}
```

`no_run`: `build` reads `CARGO_MANIFEST_DIR` and prints `cargo:` lines,
which only mean something inside a build script.

The crate has no features and no settings. One environment variable
changes the build: `DEV=1` or `DEV=true` turns off minifying, so the code
in the browser's debugger reads like its source. Any other value, or none,
minifies. The build script declares `DEV`, so a change rebuilds.

## The rules

| Input | Output |
| --- | --- |
| `assets/<dir>/_name.js`, `.ts` or `.mjs` | `assets/<dir>/name.js`, one ES module, TypeScript compiled |
| `assets/<dir>/_name.css` | `assets/<dir>/name.css`, `@import`s inlined |
| a dynamic `import()` in an entry | `public/<dir>/chunks/<name>-<hash>.js` |
| `assets/<dir>/_src/…` | nothing: modules the entries import |
| an import of a URL (`/x.js`, `https://…`) | nothing: it stays a URL |

**Reason chunks go to `public/`:** an entry imports a chunk by a relative
URL (`./chunks/bar-1a2b3c4d.js`). A fingerprinted copy in `assets/` would
break that URL. The hash in the chunk's name already changes when its
content changes.

**Limit:** a chunk cannot import its entry back, because the entry's
published name is fingerprinted. Rolldown does that when a module is both
in the entry and in a chunk. The build then fails and names both. Keep an
entry small and load the shared code with `import()`.

## Examples

### Bundle a directory

`bundle` is `build` without Cargo's environment. It takes the paths and the
minify choice, and returns what it wrote and what it read.

```rust
use std::fs;
use brocanth_bundle::{bundle, Options};

let root = std::env::temp_dir().join(format!("brocanth-bundle-readme-{}", std::process::id()));
fs::create_dir_all(root.join("assets/js/_src")).unwrap();
fs::write(root.join("assets/_site.css"), "@import './base.css';\na { color: red; }\n").unwrap();
fs::write(root.join("assets/base.css"), "b { color: blue; }\n").unwrap();
fs::write(
    root.join("assets/js/_app.ts"),
    "import { twice } from './_src/math';\nexport const four: number = twice(2);\n",
).unwrap();
fs::write(
    root.join("assets/js/_src/math.ts"),
    "export function twice(x: number): number { return x * 2; }\n",
).unwrap();

let report = bundle(&Options {
    root: root.clone(),
    assets: "assets",
    public: "public",
    minify: false,
})
.unwrap();

let css = fs::read_to_string(root.join("assets/site.css")).unwrap();
assert!(css.contains("red") && css.contains("#00f"), "the import is inlined");

let js = fs::read_to_string(root.join("assets/js/app.js")).unwrap();
assert!(js.contains("x * 2") && !js.contains(": number"), "one file, TypeScript compiled");

// Every file read, for `cargo:rerun-if-changed`.
assert!(report.sources.iter().any(|s| s.ends_with("math.ts")));
assert_eq!(report.written.len(), 2);
# fs::remove_dir_all(root).ok();
```

### Reference the output

The outputs sit in `assets/`, so a page names them by their logical path
through `brocanth-assets`:

```text
<script type="module" src={crate::assets::url("js/app.js")}></script>
<link rel="stylesheet" href={crate::assets::url("site.css")}>
```

### Add an entry

The build prints `cargo:rerun-if-changed` for every input and every module
it bundled. An edit to any of them rebuilds. A **new** entry needs
`touch build.rs`, as a new asset needs `touch src/assets.rs`.

## Testing

```sh
cargo test -p brocanth-bundle
```

No environment variable and no network are needed. The unit tests bundle a
TypeScript entry with a dynamic import and a CSS entry with an `@import`,
in a temporary directory. The README example above runs as a doctest.

An application tests the bundle through the page that loads it: with
`brocanth-test`, check that the `<script>` element's `src` is
`assets::url("js/app.js")`. Do not assert on the minified bytes. See
`crates/brocanth-test/README.md`.

## Links

- `crates/brocanth-assets/README.md`: embeds and fingerprints the outputs.
- The crate docs (`cargo doc -p brocanth-bundle --open`): the chunk rule
  and the rebuild rule in full.
