(function () {
  "use strict";
  var w = window, d = document, n = navigator;
  var data = d.getElementById("brocanth-gtag");
  if (!data) return;
  var cfg = JSON.parse(data.textContent);
  var KEY = "brocanth-consent";
  var DAY = 86400000;
  w.dataLayer = w.dataLayer || [];
  function gtag() { w.dataLayer.push(arguments); }
  if (!w.gtag) w.gtag = gtag;

  function pii(s) { return /@|%40/i.test(s); }
  function scrub(href) {
    var hash = href.indexOf("#");
    if (hash >= 0) href = href.slice(0, hash);
    var q = href.indexOf("?");
    var base = q >= 0 ? href.slice(0, q) : href;
    var query = q >= 0 ? href.slice(q + 1) : "";
    var scheme = base.indexOf("://");
    var cut = scheme >= 0 ? base.indexOf("/", scheme + 3) : 0;
    var origin = cut >= 0 ? base.slice(0, cut) : base;
    var path = cut >= 0 ? base.slice(cut) : "";
    path = path.split("/").map(function (s) { return pii(s) ? "redacted" : s; }).join("/");
    var kept = query.split("&").filter(function (pair) {
      return pair !== "" && cfg.allow.indexOf(pair.split("=")[0]) >= 0 && !pii(pair);
    });
    return origin + path + (kept.length ? "?" + kept.join("&") : "");
  }

  var signal = n.globalPrivacyControl === true || n.doNotTrack === "1" || w.doNotTrack === "1";
  var refused = cfg.gpc && signal;
  function read() {
    try {
      var saved = JSON.parse(w.localStorage.getItem(KEY));
      if (saved && saved.at + cfg.days * DAY > Date.now()) return saved.choice;
    } catch (e) {}
    return null;
  }
  function write(choice) {
    try { w.localStorage.setItem(KEY, JSON.stringify({ choice: choice, at: Date.now() })); } catch (e) {}
    if (cfg.log) report(choice);
  }
  function consentId() {
    var id = null;
    try { id = w.localStorage.getItem(KEY + "-id"); } catch (e) {}
    if (!id) {
      var bytes = new Uint8Array(16);
      w.crypto.getRandomValues(bytes);
      id = Array.prototype.map.call(bytes, function (b) { return ("0" + b.toString(16)).slice(-2); }).join("");
      try { w.localStorage.setItem(KEY + "-id", id); } catch (e) {}
    }
    return id;
  }
  function report(choice) {
    var url = "/__consent?c=" + choice + "&id=" + consentId() + "&v=" + cfg.version;
    try { w.fetch(url, { keepalive: true, credentials: "omit" }); } catch (e) {}
  }

  gtag("consent", "default", {
    ad_storage: "denied",
    ad_user_data: "denied",
    ad_personalization: "denied",
    analytics_storage: "denied",
    wait_for_update: 500
  });
  gtag("js", new Date());
  gtag("config", cfg.id, {
    send_page_view: false,
    allow_google_signals: false,
    allow_ad_personalization_signals: false,
    cookie_expires: 34128000,
    cookie_flags: "SameSite=Lax",
    cookie_domain: cfg.domain || "auto"
  });

  var loaded = false, active = false;
  function load() {
    if (loaded) return;
    loaded = true;
    var s = d.createElement("script");
    s.async = true;
    s.src = "https://www.googletagmanager.com/gtag/js?id=" + encodeURIComponent(cfg.id);
    d.head.appendChild(s);
  }
  function view() {
    if (!active) return;
    var location = scrub(w.location.href);
    gtag("event", "page_view", {
      page_location: location,
      page_referrer: d.referrer ? d.referrer.split("/").slice(0, 3).join("/") : "",
      page_title: cfg.title ? d.title : location.slice(location.indexOf("/", location.indexOf("://") + 3))
    });
  }
  function grant() {
    gtag("consent", "update", { analytics_storage: "granted" });
    active = true;
    load();
    view();
  }
  function deny() {
    gtag("consent", "update", { analytics_storage: "denied" });
    active = cfg.mode === "advanced" && !refused && loaded;
    d.cookie.split("; ").forEach(function (pair) {
      var name = pair.split("=")[0];
      if (!/^_ga/.test(name)) return;
      var host = w.location.hostname;
      var domains = ["", host];
      if (cfg.domain) {
        domains.push(cfg.domain, "." + cfg.domain.replace(/^\./, ""));
      } else {
        var labels = host.split(".");
        for (var i = 0; i < labels.length - 1; i++) domains.push("." + labels.slice(i).join("."));
      }
      domains.forEach(function (domain) {
        d.cookie = name + "=; Max-Age=0; Path=/" + (domain ? "; Domain=" + domain : "");
      });
    });
  }

  var push = w.history.pushState;
  w.history.pushState = function () {
    var out = push.apply(this, arguments);
    view();
    return out;
  };
  w.addEventListener("popstate", view);

  var banner = d.getElementById("brocanth-consent");
  function show(focus) {
    if (!banner) return;
    banner.hidden = false;
    var first = banner.querySelector("button");
    if (focus && first) first.focus();
  }
  function hide() { if (banner) banner.hidden = true; }
  if (banner) {
    banner.addEventListener("click", function (event) {
      var choice = event.target.getAttribute && event.target.getAttribute("data-consent");
      if (choice === "accept") { write("granted"); hide(); grant(); }
      if (choice === "refuse") { write("denied"); hide(); deny(); }
    });
    banner.addEventListener("keydown", function (event) {
      if (event.key === "Escape") hide();
    });
  }
  d.addEventListener("click", function (event) {
    var link = event.target.closest && event.target.closest("[data-consent-manage]");
    if (!link) return;
    event.preventDefault();
    show(true);
  });
  w.brocanthConsent = { open: function () { show(true); }, scrub: scrub };

  var choice = refused ? "refused" : read();
  if (choice === "granted") {
    grant();
  } else if (cfg.mode === "advanced" && !refused) {
    active = true;
    load();
    view();
  }
  if (choice === null) show(false);
})();
