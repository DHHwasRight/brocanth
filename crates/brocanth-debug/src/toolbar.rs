//! [`DebugToolbar`]: the request hook that starts the clock, and the
//! response hook that writes the panel.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use brocanth::plugin_api::{Lifecycle, Plugin, PluginError};
use brocanth::plugin_context::NoContext;
use brocanth::plugin_hooks::{
    BodyShape, Both, Flow, HookNames, HookRegistrar, HookSource, OnRequest, OnResponse, Pipeline,
    Request, RequestHook, RequestState, ResponseHead, ResponseHook,
};
use brocanth::plugin_order::{Order, Phase};

/// Whether this build may show the panel: `cfg!(debug_assertions)`.
///
/// False in a release build. Then the plugin installs no hook and registers
/// no probe, whatever `enabled` says.
pub const ACTIVE: bool = cfg!(debug_assertions);

/// The toolbar settings: `[debug_toolbar]` in a config file.
#[derive(Clone, Debug)]
#[cfg_attr(
    feature = "config",
    derive(serde::Serialize, serde::Deserialize),
    serde(default, deny_unknown_fields)
)]
pub struct ToolbarConfig {
    /// Show the panel. Off, the hooks are installed and do nothing.
    pub enabled: bool,
    /// The most SQL statement shapes the panel lists. The count is always
    /// the full one.
    pub max_statements: usize,
}

impl Default for ToolbarConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_statements: 50,
        }
    }
}

/// What the application told the toolbar once it was built: the hook
/// chains and the settings report.
#[derive(Debug)]
struct Observed {
    names: HookNames,
    off: Vec<String>,
    settings: String,
}

/// The toolbar. Register it with `plugin_hooking`, then call
/// [`DebugToolbar::observe`] once the application is built.
pub struct DebugToolbar {
    config: ToolbarConfig,
    observed: Arc<OnceLock<Observed>>,
    #[cfg(feature = "model")]
    probe: brocanth_model::Probe,
}

impl DebugToolbar {
    /// Whether this instance writes a panel: a debug build, and `enabled`.
    pub fn active(&self) -> bool {
        ACTIVE && self.config.enabled
    }

    /// Hands the toolbar what only the built application knows: the hook
    /// chains, and the settings report (`crate::plugins::settings_report()`
    /// in a generated project). Called once; a second call changes nothing.
    pub fn observe(&self, pipeline: &Pipeline, settings: Result<String, String>) {
        let off = pipeline
            .skipped()
            .iter()
            .map(|(instance, name)| format!("{instance}.{name}"))
            .collect();
        let settings = settings.unwrap_or_else(|e| format!("the settings report failed: {e}"));
        let _ = self.observed.set(Observed {
            names: pipeline.hook_names().clone(),
            off,
            settings,
        });
    }

    /// The statement probe the ORM reports to.
    #[cfg(feature = "model")]
    pub fn probe(&self) -> &brocanth_model::Probe {
        &self.probe
    }
}

impl Plugin for DebugToolbar {
    type Config = ToolbarConfig;
    type Provides = NoContext;
    /// Also the configuration section.
    const NAME: &'static str = "debug_toolbar";
    const INSTALLS_HOOKS: bool = true;

    fn new(config: ToolbarConfig) -> Self {
        Self {
            config,
            observed: Arc::new(OnceLock::new()),
            #[cfg(feature = "model")]
            probe: brocanth_model::Probe::new(brocanth_model::ProbeConfig::default()),
        }
    }

    fn register(&mut self, lc: &mut Lifecycle<'_>) -> Result<(), PluginError> {
        // The model plugin reads it in `before_start` and installs it on the
        // ORM's connection. Only when the panel shows: a release build
        // counts no statement.
        #[cfg(feature = "model")]
        if self.active() {
            lc.services_mut().insert(self.probe.clone());
        }
        let _ = lc;
        Ok(())
    }
}

impl HookSource for DebugToolbar {
    type Families = Both<OnRequest, OnResponse>;
    /// Both install only in a debug build with `enabled` on.
    const HOOKS: &'static [&'static str] = &["start", "panel"];

    fn install_hooks(&self, registrar: &mut HookRegistrar<'_>) -> Result<(), PluginError> {
        if !self.active() {
            return Ok(());
        }
        registrar
            .request(
                "start",
                Phase::Identify,
                Order::new().first(),
                Start {
                    #[cfg(feature = "model")]
                    probe: self.probe.clone(),
                },
            )
            .response(
                "panel",
                Phase::Identify,
                Order::new(),
                Panel {
                    config: self.config.clone(),
                    observed: Arc::clone(&self.observed),
                },
            );
        Ok(())
    }
}

/// What the request hook leaves for the response hook.
struct Started {
    at: Instant,
    #[cfg(feature = "model")]
    unit: brocanth_model::UnitOfWork,
}

struct Start {
    #[cfg(feature = "model")]
    probe: brocanth_model::Probe,
}

impl RequestHook for Start {
    fn before_dispatch(
        &self,
        request: &mut Request,
        state: &mut RequestState,
    ) -> Result<Flow, PluginError> {
        let _ = &request;
        state.insert(Started {
            at: Instant::now(),
            #[cfg(feature = "model")]
            unit: self
                .probe
                .unit_of_work(format!("{} {}", request.method(), request.uri().path())),
        });
        Ok(Flow::Continue)
    }
}

struct Panel {
    /// Read by the SQL section, which needs the ORM.
    #[cfg_attr(not(feature = "model"), allow(dead_code))]
    config: ToolbarConfig,
    observed: Arc<OnceLock<Observed>>,
}

impl ResponseHook for Panel {
    fn on_headers(
        &self,
        _request: &Request,
        _head: &mut ResponseHead,
        _body: BodyShape<'_>,
        _state: &mut RequestState,
    ) -> Result<(), PluginError> {
        Ok(())
    }

    fn on_body(
        &self,
        request: &Request,
        head: &mut ResponseHead,
        body: &mut Vec<u8>,
        state: &mut RequestState,
    ) -> Result<(), PluginError> {
        let html = brocanth::plugin_body::is_html(head.headers.get(http::header::CONTENT_TYPE));
        if !html || request.headers().contains_key("dp-partial") {
            return Ok(());
        }
        let markup = self.markup(request, head, state);
        brocanth::plugin_body::insert_before_body_end(body, markup.as_bytes());
        Ok(())
    }
}

impl Panel {
    fn markup(&self, request: &Request, head: &ResponseHead, state: &RequestState) -> String {
        let started = state.get::<Started>();
        let elapsed = started.map(|s| s.at.elapsed().as_secs_f64() * 1000.0);
        let (sql, count) = self.sql(started);

        let mut out = String::from(
            "<details id=\"brocanth-debug-toolbar\" style=\"position:fixed;right:0;bottom:0;\
             z-index:2147483647;max-width:min(60em,100vw);max-height:70vh;overflow:auto;\
             background:#111;color:#eee;font:12px/1.4 ui-monospace,monospace;padding:4px 8px\">",
        );
        let _ = write!(
            out,
            "<summary>debug: {} {} · {}",
            escape(request.method().as_str()),
            escape(request.uri().path()),
            head.status.as_u16()
        );
        if let Some(ms) = elapsed {
            let _ = write!(out, " · {ms:.1} ms");
        }
        if let Some(n) = count {
            let _ = write!(out, " · {n} SQL");
        }
        out.push_str("</summary>");
        out.push_str(&sql);
        out.push_str("<h2>Hooks</h2>");
        match self.observed.get() {
            Some(seen) => {
                for (chain, names) in [
                    ("request", &seen.names.request),
                    ("response", &seen.names.response),
                    ("error", &seen.names.error),
                ] {
                    let _ = write!(out, "<p>{chain}: {}</p>", list(names));
                }
                let _ = write!(out, "<p>off: {}</p>", list(&seen.off));
                let _ = write!(
                    out,
                    "<h2>Settings</h2><pre>{}</pre>",
                    escape(&seen.settings)
                );
            }
            None => out.push_str(
                "<p>Not known: the application did not call <code>DebugToolbar::observe</code>.</p>",
            ),
        }
        out.push_str("<p>Development only: a release build shows no panel.</p></details>");
        out
    }
}

impl Panel {
    /// The SQL section, and the statement count for the summary.
    #[cfg(feature = "model")]
    fn sql(&self, started: Option<&Started>) -> (String, Option<usize>) {
        let Some(started) = started else {
            return (String::new(), None);
        };
        let shapes = started.unit.shapes();
        let total = started.unit.statements();
        let mut sql = String::new();
        let _ = write!(
            sql,
            "<h2>SQL: {total} statement{}, {} shape{}</h2><ol>",
            plural(total),
            shapes.len(),
            plural(shapes.len())
        );
        for shape in shapes.iter().take(self.config.max_statements) {
            let _ = write!(
                sql,
                "<li>{}× <code>{}</code></li>",
                shape.count,
                escape(&shape.shape)
            );
        }
        sql.push_str("</ol>");
        if shapes.len() > self.config.max_statements {
            let _ = write!(
                sql,
                "<p>{} more shapes; raise <code>max_statements</code> under [debug_toolbar].</p>",
                shapes.len() - self.config.max_statements
            );
        }
        (sql, Some(total))
    }

    /// Without the ORM there is no statement to count.
    #[cfg(not(feature = "model"))]
    fn sql(&self, _started: Option<&Started>) -> (String, Option<usize>) {
        (
            "<h2>SQL</h2><p>Not counted: the ORM is not in this project. \
             <code>brocanth add model</code> counts its statements.</p>"
                .to_string(),
            None,
        )
    }
}

#[cfg(feature = "model")]
fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn list(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names
            .iter()
            .map(|n| escape(n))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Text into HTML, for the five characters that matter in an element and
/// an attribute.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use brocanth::plugin_app::AppBuilder;
    use brocanth::plugin_body::Body;
    use brocanth::plugin_hooks::{HandlerError, Response};

    fn page(_rq: &mut Request, _st: &mut RequestState) -> Result<Response, HandlerError> {
        let mut response = Response::new(Body::from(
            "<!doctype html><title>t</title><body><p>hi</p></body>",
        ));
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("text/html; charset=utf-8"),
        );
        Ok(response)
    }

    fn json(_rq: &mut Request, _st: &mut RequestState) -> Result<Response, HandlerError> {
        let mut response = Response::new(Body::from("{\"a\":1}"));
        response.headers_mut().insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/json"),
        );
        Ok(response)
    }

    fn body(response: Response) -> String {
        String::from_utf8(response.into_body().into_buffered().expect("a whole body"))
            .expect("utf-8")
    }

    fn request(path: &str) -> Request {
        http::Request::builder()
            .uri(path)
            .body(Vec::new())
            .expect("a request")
    }

    #[test]
    fn an_html_page_gets_the_panel_with_its_timing_hooks_and_settings() {
        let toolbar = DebugToolbar::new(ToolbarConfig::default());
        let app = AppBuilder::new()
            .plugin_hooking(toolbar)
            .build()
            .expect("an application");
        let plugin = app.plugin::<DebugToolbar>().expect("registered");
        plugin.observe(app.pipeline(), Ok("debug_toolbar.enabled = true".into()));
        let text = body(app.run(&mut request("/a%3Cb"), &mut RequestState::new(), page));
        assert!(text.contains("debug: GET /a%3Cb · 200 · "), "{text}");
        assert!(text.contains(" ms"), "{text}");
        assert!(text.contains("request: debug_toolbar.start"), "{text}");
        assert!(text.contains("response: debug_toolbar.panel"), "{text}");
        assert!(text.contains("debug_toolbar.enabled = true"), "{text}");
        // Before `</body>`, so the page stays one document.
        assert!(text.ends_with("</details></body>"), "{text}");
    }

    #[test]
    fn json_a_fragment_and_a_turned_off_toolbar_are_left_alone() {
        let app = AppBuilder::new()
            .plugin_hooking(DebugToolbar::new(ToolbarConfig::default()))
            .build()
            .expect("an application");
        assert_eq!(
            body(app.run(&mut request("/"), &mut RequestState::new(), json)),
            "{\"a\":1}"
        );
        let mut partial = request("/");
        partial
            .headers_mut()
            .insert("dp-partial", http::HeaderValue::from_static("1"));
        assert!(!body(app.run(&mut partial, &mut RequestState::new(), page)).contains("debug:"));

        let off = AppBuilder::new()
            .plugin_hooking(DebugToolbar::new(ToolbarConfig {
                enabled: false,
                ..ToolbarConfig::default()
            }))
            .build()
            .expect("an application");
        assert!(off.pipeline().hook_names().request.is_empty());
        assert!(
            !body(off.run(&mut request("/"), &mut RequestState::new(), page)).contains("debug:")
        );
    }

    #[test]
    fn text_from_the_request_and_the_settings_is_escaped() {
        assert_eq!(escape("<script>\"'&"), "&lt;script&gt;&quot;&#39;&amp;");
    }

    #[test]
    fn this_test_build_is_one_that_may_show_the_panel() {
        // `cargo test` builds with debug assertions; `--release` would not,
        // and then no hook is installed (the test above would fail loudly).
        assert_eq!(ACTIVE, cfg!(debug_assertions));
    }
}
