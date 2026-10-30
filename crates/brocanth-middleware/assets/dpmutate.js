// dp-mutate: the mutating half of dp-swap.
//
// dp-swap fetches with GET and nothing else. This file adds `dp-post`,
// `dp-put`, `dp-patch` and `dp-delete`, and it uses dp-swap's own swap,
// out-of-band and settle functions, so a write swaps exactly as a read does.
//
//   <form method="post" action="/cart/items" dp-post dp-target="#cart">
//   <button dp-delete="/cart/items/7" dp-target="closest li" dp-swap="delete">
//
// The rules:
//
// - The attribute value is the URL. An empty value takes the form's `action`.
// - A form sends its own fields; any other element sends the fields of the
//   form around it, and its own `name` and `value`.
// - `dp-target`, `dp-swap` and `dp-select` mean what they mean to dp-swap,
//   and are inherited the same way. `dp-confirm` asks first.
// - The CSRF token goes in a header. It comes from
//   `<meta name="csrf-token" content=".." data-header="x-csrf-token">`, or
//   from the `csrf_token` cookie when the page has no meta tag.
// - A 2xx swaps. A 422 swaps too: it is the form again, with its errors.
//   Any other status swaps nothing and fires `dp:responseError`.
// - The response may steer the swap with `DP-Redirect`, `DP-Refresh`,
//   `DP-Retarget`, `DP-Reswap` and `DP-Trigger`, and may carry
//   `dp-swap-oob` elements, which dp-swap applies by id.
//
// The listeners are on the document, so content swapped in later needs no
// processing. Without dp-swap on the page, nothing is intercepted and the
// form submits as a plain form, which is the reason every example above
// keeps its `method` and `action`.
(() => {
  "use strict";
  const g = globalThis;
  const doc = g.document;
  if (!doc || g.dpmutate) return;

  const VERBS = ["post", "put", "patch", "delete"];
  const SELECTOR = VERBS.map((v) => `[dp-${v}]`).join(",");
  const STYLES = [
    "innerHTML", "outerHTML", "beforebegin", "afterbegin",
    "beforeend", "afterend", "delete", "none",
  ];
  const config = {
    csrfMeta: "csrf-token",
    csrfCookie: "csrf_token",
    csrfHeader: "x-csrf-token",
    requestClass: "dp-request",
    timeout: 10000,
    fetch: (...args) => g.fetch(...args),
  };
  const inflight = new WeakSet();

  const verbOf = (el) => VERBS.find((v) => el.hasAttribute(`dp-${v}`)) ?? null;

  function inherited(el, name) {
    const at = el.closest(`[dp-${name}]`);
    return at ? at.getAttribute(`dp-${name}`) : null;
  }

  // dp-swap's target language: this, closest X, find X, next, previous, or
  // a selector over the document.
  function resolve(el, spec) {
    if (!spec || spec === "this") return el;
    if (spec.startsWith("closest ")) return el.closest(spec.slice(8).trim());
    if (spec.startsWith("find ")) return el.querySelector(spec.slice(5).trim());
    if (spec === "next") return el.nextElementSibling;
    if (spec === "previous") return el.previousElementSibling;
    return doc.querySelector(spec);
  }

  function parseSwap(raw) {
    const swap = { style: "innerHTML", scroll: null, delay: 0 };
    for (const token of (raw ?? "").trim().split(/\s+/)) {
      if (STYLES.includes(token)) swap.style = token;
      else if (token === "scroll:top" || token === "scroll:bottom") swap.scroll = token.slice(7);
    }
    return swap;
  }

  function emit(el, name, detail) {
    const at = el.isConnected ? el : doc.documentElement;
    return at.dispatchEvent(new CustomEvent(name, { detail, bubbles: true, cancelable: true }));
  }

  function cookie(name) {
    for (const pair of doc.cookie.split(";")) {
      const [key, ...rest] = pair.split("=");
      if (key.trim() === name) return decodeURIComponent(rest.join("=").trim());
    }
    return null;
  }

  function fields(source, form, submitter) {
    let data;
    try {
      data = form ? new FormData(form, submitter ?? undefined) : new FormData();
    } catch {
      data = new FormData(form);
    }
    if (source !== form && source !== submitter && source.name) {
      data.append(source.name, source.value ?? "");
    }
    return data;
  }

  // The token goes in a header, so a multipart body that the server hook
  // will not parse still carries it.
  function csrf(headers, data) {
    const meta = doc.querySelector(`meta[name="${config.csrfMeta}"]`);
    if (meta) {
      const header = meta.getAttribute("data-header");
      const field = meta.getAttribute("data-field");
      if (header) headers[header] = meta.content;
      else if (field && !data.has(field)) data.append(field, meta.content);
      return;
    }
    const held = cookie(config.csrfCookie);
    if (held) headers[config.csrfHeader] = held;
  }

  function encode(form, data) {
    const files = [...data.values()].some((v) => typeof v !== "string");
    if (files || form?.enctype === "multipart/form-data") return data;
    return new URLSearchParams(data);
  }

  function specOf(source, form, submitter) {
    const verb = verbOf(source);
    const target = resolve(source, inherited(source, "target"));
    if (!verb || !target) return null;
    return {
      source,
      verb: verb.toUpperCase(),
      url: source.getAttribute(`dp-${verb}`) || form?.getAttribute("action") || g.location.href,
      target,
      swap: parseSwap(inherited(source, "swap")),
      select: inherited(source, "select"),
      confirm: source.getAttribute("dp-confirm"),
      data: fields(source, form, submitter),
      form,
    };
  }

  async function send(spec) {
    const { source } = spec;
    if (inflight.has(source)) return;
    if (spec.confirm && !g.confirm(spec.confirm)) return;
    if (!emit(source, "dp:beforeRequest", { spec })) return;
    const headers = { "DP-Request": "true" };
    if (spec.target.id) headers["DP-Target"] = spec.target.id;
    csrf(headers, spec.data);
    // A row that is about to go away needs no page back. A redirect is
    // then the answer "done", and following it would render a page that
    // is thrown away.
    const quiet = spec.swap.style === "delete" || spec.swap.style === "none";
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), config.timeout);
    inflight.add(source);
    source.classList.add(config.requestClass);
    source.setAttribute("aria-busy", "true");
    try {
      const response = await config.fetch(spec.url, {
        method: spec.verb,
        headers,
        body: encode(spec.form, spec.data),
        credentials: "same-origin",
        redirect: quiet ? "manual" : "follow",
        signal: abort.signal,
      });
      await answer(spec, response);
    } catch (error) {
      emit(source, "dp:sendError", { spec, error });
    } finally {
      clearTimeout(timer);
      inflight.delete(source);
      source.classList.remove(config.requestClass);
      source.removeAttribute("aria-busy");
      emit(source, "dp:afterRequest", { spec });
    }
  }

  async function answer(spec, response) {
    const header = (name) => response.headers.get(name);
    const away = header("DP-Redirect");
    if (away) return g.location.assign(away);
    if (header("DP-Refresh") === "true") return g.location.reload();

    const opaque = response.type === "opaqueredirect";
    if (!opaque && !response.ok && response.status !== 422) {
      emit(spec.source, "dp:responseError", { spec, response });
      return;
    }
    if (response.status === 422) emit(spec.source, "dp:invalid", { spec, response });

    const retarget = header("DP-Retarget");
    const target = (retarget && doc.querySelector(retarget)) || spec.target;
    const swap = header("DP-Reswap") ? parseSwap(header("DP-Reswap")) : spec.swap;
    const text = opaque || response.status === 204 ? "" : await response.text();

    const s = g.dpswap.swapping;
    const fragment = s.parseFragment(doc, text);
    s.applyOutOfBand(doc, fragment);
    const content = s.selectFrom(doc, fragment, spec.select);
    const empty = (opaque || response.status === 204) && swap.style !== "delete";
    if (!empty && emit(target, "dp:beforeSwap", { spec, content })) {
      const inside = ["innerHTML", "afterbegin", "beforeend", "none"].includes(swap.style);
      const settled = inside ? target : (target.parentElement ?? doc.documentElement);
      s.swapInto(target, content, swap);
      // dp-swap listens for this and enhances whatever arrived.
      emit(settled, "dp:afterSwap", { spec });
      s.settle(target, swap);
      emit(settled, "dp:afterSettle", { spec });
    }
    trigger(spec.source, header("DP-Trigger"));
  }

  function trigger(source, raw) {
    if (!raw) return;
    if (raw.trim().startsWith("{")) {
      try {
        for (const [name, detail] of Object.entries(JSON.parse(raw))) emit(source, name, detail);
      } catch {}
      return;
    }
    for (const name of raw.split(",")) if (name.trim()) emit(source, name.trim(), {});
  }

  const ready = () => Boolean(g.dpswap?.swapping);

  const submits = (el) => {
    const tag = el.tagName;
    const type = (el.getAttribute("type") ?? "").toLowerCase();
    return (tag === "BUTTON" && (type === "" || type === "submit")) ||
      (tag === "INPUT" && (type === "submit" || type === "image"));
  };

  doc.addEventListener("submit", (event) => {
    if (event.defaultPrevented || !ready()) return;
    const form = event.target;
    const submitter = event.submitter ?? null;
    const source = submitter && verbOf(submitter) ? submitter : verbOf(form) ? form : null;
    const spec = source && specOf(source, form, submitter);
    if (!spec) return;
    event.preventDefault();
    void send(spec);
  });

  doc.addEventListener("click", (event) => {
    if (event.defaultPrevented || event.button !== 0 || !ready()) return;
    if (!(event.target instanceof Element)) return;
    const source = event.target.closest(SELECTOR);
    // A form, and a button that submits one, are the submit listener's.
    if (!source || source.tagName === "FORM" || (source.form && submits(source))) return;
    const spec = specOf(source, source.closest("form"), null);
    if (!spec) return;
    event.preventDefault();
    void send(spec);
  });

  g.dpmutate = {
    config,
    send: (el) => {
      const spec = specOf(el, el.closest("form"), null);
      return spec ? send(spec) : Promise.resolve();
    },
  };
})();
