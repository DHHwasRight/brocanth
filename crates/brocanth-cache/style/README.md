# Rebuilding the panel's stylesheet

`src/admin.css` is a build artefact. It is committed and the panel inlines
it, so an application that mounts the cache tab needs no node and no CSS
build of its own.

```sh
cd crates/brocanth-cache
npm install      # once
npm run css      # or `npm run css:watch` while working
```

## Why the panel has its own, and not the admin's

A `Panel` is contributed by another crate. The admin's Tailwind build scans
the admin's templates; it cannot see class names in this crate, and making
it try would mean a panel could only be written by somebody who could also
edit `brocanth-admin`.

So each side ships what it uses. The admin's sheet carries the preflight
and resets the document; this one carries no preflight, only the theme
variables and the utilities this template names. Two resets would fight.

The tokens are duplicated from `brocanth-admin/style/admin.css` on purpose.
Importing them would mean reaching into another crate's source, which
breaks the moment that dependency is optional — and here it is.
