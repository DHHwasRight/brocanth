use crate::admin::TabBar;

// The panel's own tab strip, and the only stylesheet it brings.
//
// Inline rather than a file the admin has to serve: a panel renders a body
// and nothing else, and asking the admin to mount an asset route for one
// contributed tab would be a seam widened for a stylesheet. Every class
// below is prefixed, so it cannot reach the admin's own chrome.

pub fn Tabs(tabs: &TabBar) -> Element {
    <style>{! STYLE }</style>
    <nav class="jobs-tabs">
        for tab in &tabs.tabs {
            <a href={&tab.href} class={tab_class(tab.current)}>{&tab.label}</a>
        }
    </nav>
}

fn tab_class(current: bool) -> &'static str {
    if current {
        "jobs-tab is-current"
    } else {
        "jobs-tab"
    }
}

const STYLE: &str = "
.jobs-tabs{display:flex;gap:4px;margin:0 0 18px;border-bottom:1px solid #e1e4ea}
.jobs-tab{padding:7px 13px;color:#4b5563;border-bottom:2px solid transparent;margin-bottom:-1px}
.jobs-tab:hover{color:#1d1f23;text-decoration:none}
.jobs-tab.is-current{color:#2c5fd6;border-bottom-color:#2c5fd6;font-weight:600}
.jobs-counts{display:grid;grid-template-columns:repeat(auto-fit,minmax(130px,1fr));gap:10px;margin-bottom:18px}
.jobs-count{display:block;background:#fff;border:1px solid #e1e4ea;border-radius:8px;padding:14px;color:#1d1f23}
.jobs-count:hover{border-color:#2c5fd6;text-decoration:none}
.jobs-count-value{display:block;font-size:24px;font-weight:600;line-height:1.1}
.jobs-count-label{display:block;font-size:12px;color:#6b7280;text-transform:uppercase;letter-spacing:.02em;margin-top:2px}
.jobs-err{color:#8d2a20;font-size:12px;display:block;max-width:46ch;overflow-wrap:anywhere}
.jobs-act{display:inline;margin:0}
.jobs-act button{background:none;border:0;padding:0;font:inherit;color:#2c5fd6;cursor:pointer}
.jobs-act button:hover{text-decoration:underline}
.jobs-act button.danger{color:#c0392b}
.jobs-stale{color:#c0392b;font-weight:600}
";
