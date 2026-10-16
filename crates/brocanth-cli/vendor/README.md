# Vendored, not fetched

`dpswap.min.js` is a build artefact of <https://github.com/brocanth/htmx>,
copied here at commit `f80aeeb` and embedded in the `brocanth` binary with
`include_str!`.

Vendored rather than fetched, for the reason the rest of this tool is built
the way it is: `brocanth create --with rsx,rsx-htmx` has to work on a machine with
no network and no node, and a scaffold that sometimes produces a project
missing a file is worse than one that is a few kilobytes larger.

To update it: build `dist/dpswap.min.js` in that checkout, copy it here, and
change the commit above. Nothing reads the commit — it is for whoever asks
which version this is.
