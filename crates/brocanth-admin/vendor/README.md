# Vendored, not fetched

`dpswap.min.js` is the same build artefact `brocanth-cli/vendor/` carries,
from <https://github.com/brocanth/htmx> at commit `f80aeeb`, copied here and
embedded in the layout with `include_str!`.

A second copy rather than a shared crate, and the reason is that the two
uses have nothing to do with each other. The CLI writes the file into a
generated project, where it is a file the developer owns and may replace.
The admin inlines it into a page it renders, where nobody can. A crate
holding both would exist to avoid duplicating thirteen kilobytes and would
couple a generator to a backoffice to do it.

Update both when you update either, and say so in the commit.
