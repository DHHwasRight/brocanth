// dp-prefetch: fetch a page before the click, the way Astro's prefetch does.
//
//   <a href="/posts/7" dp-prefetch>                 hover (the default)
//   <a href="/posts/7" dp-prefetch="tap">           on pointerdown or touchstart
//   <nav dp-prefetch="viewport"> <a href=..> </nav> when a link scrolls into view
//   <a href="/pricing" dp-prefetch="load">          when the page has loaded
//   <html dp-prefetch="hover">                      every link on the page
//   <a href="/logout" dp-prefetch="false">          never this one
//
// The rules:
//
// - The strategy is inherited, like `dp-target`: the nearest ancestor with
//   `dp-prefetch` decides, and `false` stops it.
// - Only a GET link is prefetched: an `<a href>` on this origin, over http
//   or https, that is not a `download` and carries no `dp-post`, `dp-put`,
//   `dp-patch` or `dp-delete`. A link to this page with a different `#hash`
//   is not prefetched: it does not fetch anything.
// - Nothing is prefetched when the browser asks to save data
//   (`navigator.connection.saveData`, the `Save-Data` header's source) or
//   reports a 2g connection.
// - Each URL is prefetched once per page.
// - The request is `<link rel="prefetch">` where the browser supports it,
//   which puts the page in the HTTP cache for the navigation that follows.
//   Elsewhere it is a low-priority `fetch` with the page's cookies.
//
// What it does not do: a dp-swap request sends `DP-Request` and the server
// answers it with `Vary`, so a prefetched whole page does not answer a
// dp-swap partial request. Prefetch helps a plain navigation, and a boosted
// link that falls back to one.
(() => {
  // A layout and a screen can both carry this script. Listen once.
  if (window.__dpPrefetch) return;
  window.__dpPrefetch = true;

  "use strict";
  const g = globalThis;
  const doc = g.document;
  if (!doc) return;

  const STRATEGIES = ["hover", "tap", "viewport", "load"];
  const WRITES = ["dp-post", "dp-put", "dp-patch", "dp-delete"];
  const config = {
    // Astro's number: long enough that a pointer crossing a menu does not
    // fetch every item, short enough to land before the click.
    hoverDelay: 80,
    fetch: (...args) => g.fetch(...args),
  };
  const done = new Set();
  const timers = new WeakMap();

  // The strategy for a link, or null. An empty attribute means hover.
  function strategyOf(el) {
    const at = el.closest("[dp-prefetch]");
    if (!at) return null;
    const value = (at.getAttribute("dp-prefetch") ?? "").trim().toLowerCase();
    if (value === "") return "hover";
    return STRATEGIES.includes(value) ? value : null;
  }

  function saving() {
    const c = g.navigator?.connection;
    if (!c) return false;
    return c.saveData === true || /2g/.test(c.effectiveType ?? "");
  }

  // The URL to prefetch, or null when the link is not one to prefetch.
  function urlOf(el) {
    if (!el || el.tagName !== "A" || !el.hasAttribute("href")) return null;
    if (el.hasAttribute("download")) return null;
    if (WRITES.some((w) => el.hasAttribute(w))) return null;
    let url;
    try {
      url = new URL(el.getAttribute("href"), g.location.href);
    } catch {
      return null;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return null;
    if (url.origin !== g.location.origin) return null;
    const here = new URL(g.location.href);
    if (url.pathname === here.pathname && url.search === here.search) return null;
    url.hash = "";
    return url.href;
  }

  function prefetch(href) {
    if (!href || done.has(href) || saving()) return false;
    done.add(href);
    const link = doc.createElement("link");
    if (link.relList?.supports?.("prefetch")) {
      link.rel = "prefetch";
      link.href = href;
      doc.head.append(link);
    } else {
      config
        .fetch(href, { credentials: "same-origin", priority: "low" })
        .catch(() => done.delete(href));
    }
    return true;
  }

  function linkFrom(event) {
    const t = event.target;
    return t instanceof Element ? t.closest("a[href]") : null;
  }

  function when(strategy, el, run) {
    const link = el && strategyOf(el) === strategy ? el : null;
    if (link) run(link);
  }

  // Hover, and keyboard focus, which is hover for somebody without a mouse.
  for (const type of ["mouseover", "focusin"]) {
    doc.addEventListener(type, (event) => {
      when("hover", linkFrom(event), (link) => {
        clearTimeout(timers.get(link));
        timers.set(link, setTimeout(() => prefetch(urlOf(link)), config.hoverDelay));
      });
    }, { passive: true });
  }
  for (const type of ["mouseout", "focusout"]) {
    doc.addEventListener(type, (event) => {
      const link = linkFrom(event);
      if (link) clearTimeout(timers.get(link));
    }, { passive: true });
  }

  // Tap: the press, which comes about 100ms before the click. Hover links
  // get it too, because a touch screen has no hover.
  for (const type of ["touchstart", "mousedown"]) {
    doc.addEventListener(type, (event) => {
      const link = linkFrom(event);
      const s = link && strategyOf(link);
      if (s === "tap" || s === "hover") prefetch(urlOf(link));
    }, { passive: true });
  }

  const observer = "IntersectionObserver" in g
    ? new g.IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        observer.unobserve(entry.target);
        prefetch(urlOf(entry.target));
      }
    })
    : null;

  const idle = g.requestIdleCallback ?? ((f) => setTimeout(f, 1));

  // Viewport and load links in `root`. Run on load and after each swap, so
  // links that dp-swap brings in are seen too.
  function scan(root) {
    if (saving()) return;
    for (const link of (root ?? doc).querySelectorAll("a[href]")) {
      const s = strategyOf(link);
      if (s === "viewport" && observer) observer.observe(link);
      else if (s === "load") idle(() => prefetch(urlOf(link)));
    }
  }

  if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", () => scan());
  else scan();
  doc.addEventListener("dp:afterSettle", (event) => {
    scan(event.target instanceof Element ? event.target : doc);
  });

  g.dpprefetch = { config, prefetch, scan, urlOf, strategyOf };
})();
