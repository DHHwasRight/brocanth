# Rebuilding the admin's stylesheet

`src/admin.css` is a build artefact. It is committed, and the layout embeds
it with `include_str!`, so an application that mounts the admin needs no
node, no network and no CSS build of its own.

Rebuild it after changing a class name in `src/templates/`:

```sh
cd crates/brocanth-admin
npm install      # once
npm run css      # or `npm run css:watch` while working
```

Then commit `src/admin.css` and say in the message that it is regenerated.

`node_modules/` is gitignored and `package.json` is a maintainer's tool.
Not `npx`: Tailwind resolves `@import "tailwindcss"` from the directory of
the input file, so the package has to be reachable from here.

## Why it is not built in the application

This crate's templates live in this crate. A Tailwind build in an
application scans the application's own sources; reaching these would mean
a `@source` glob pointing into the cargo registry, which breaks the first
time the version changes. The same argument as `dpswap.min.js`: vendored,
not fetched.

## Why a panel brings its own

A `Panel` is contributed by another crate and its class names are not in
this build. A panel emits its own `<style>`, which is why
`brocanth-jobs/style/` exists and looks like this directory.
