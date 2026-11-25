# brocanth-debug

A panel at the bottom of each HTML page, in development only:

- the request: verb, path, status, and the time from the first request
  hook to the response hooks;
- the SQL statements the ORM issued for it, grouped by shape with a count,
  so an N+1 shows as one shape issued many times (feature `model`);
- the hooks in each chain, in the order they run, and the hooks the
  module files turned off;
- the settings in effect, every layer, with secrets masked.

In a release build (`debug_assertions` off) the plugin installs no hook and
registers no probe: the panel cannot appear. `brocanth check --deploy`
refuses the module as well.

A generated project gets it with `brocanth add debug debug-toolbar`
(`docs/modules/debug-toolbar.md`).
