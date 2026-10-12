use crate::view::LayoutCtx;

// The shell every screen is wrapped in. It is a `View` like any other, so an
// application that wants its own chrome replaces this one and keeps the six
// screens, or replaces a screen and keeps this.

pub fn Layout(ctx: &LayoutCtx) -> Element {
    <!DOCTYPE html>
    <html lang="en">
        <head>
            <meta charset="utf-8" />
            <meta name="viewport" content="width=device-width, initial-scale=1" />
            <title>{page_title(ctx)}</title>
            <style>{! STYLE }</style>
        </head>
        <body>
            <aside class="rail">
                <a class="brand" href={&ctx.chrome.base_path}>{&ctx.chrome.title}</a>
                <nav class="rail-nav">
                    for item in &ctx.chrome.nav {
                        <a href={&item.href} class={nav_class(item.current)}>{&item.label}</a>
                    }
                </nav>
            </aside>
            <main class="stage">{! &ctx.body }</main>
        </body>
    </html>
}

fn page_title(ctx: &LayoutCtx) -> String {
    if ctx.heading.is_empty() {
        ctx.chrome.title.clone()
    } else {
        format!("{} · {}", ctx.heading, ctx.chrome.title)
    }
}

fn nav_class(current: bool) -> &'static str {
    if current {
        "rail-link is-current"
    } else {
        "rail-link"
    }
}

const STYLE: &str = "
*{box-sizing:border-box}
body{margin:0;display:flex;min-height:100vh;font:14px/1.5 ui-sans-serif,system-ui,sans-serif;color:#1d1f23;background:#f4f5f7}
a{color:#2c5fd6;text-decoration:none}
a:hover{text-decoration:underline}
.rail{width:220px;flex:0 0 220px;background:#1d2330;color:#cfd6e4;padding:20px 0;display:flex;flex-direction:column;gap:18px}
.brand{display:block;padding:0 20px;color:#fff;font-weight:600;font-size:16px}
.rail-nav{display:flex;flex-direction:column}
.rail-link{padding:7px 20px;color:#cfd6e4}
.rail-link:hover{background:#262e3e;text-decoration:none}
.rail-link.is-current{background:#2c5fd6;color:#fff}
.stage{flex:1;padding:28px 32px;min-width:0}
h1{font-size:22px;margin:0 0 4px}
h2{font-size:16px;margin:24px 0 8px}
.crumbs{font-size:12px;color:#6b7280;margin-bottom:14px}
.bar{display:flex;align-items:center;gap:12px;margin-bottom:16px;flex-wrap:wrap}
.bar-fill{flex:1}
.card{background:#fff;border:1px solid #e1e4ea;border-radius:8px;overflow:hidden}
table{width:100%;border-collapse:collapse;background:#fff}
th,td{text-align:left;padding:9px 12px;border-bottom:1px solid #eceef2;vertical-align:top}
th{background:#fafbfc;font-weight:600;font-size:12px;letter-spacing:.02em;text-transform:uppercase;color:#6b7280}
tbody tr:hover{background:#f8fafc}
.empty{padding:28px;text-align:center;color:#6b7280;background:#fff}
.btn{display:inline-block;padding:7px 13px;border-radius:6px;border:1px solid #d3d8e0;background:#fff;color:#1d1f23;cursor:pointer;font:inherit}
.btn:hover{background:#f2f4f7;text-decoration:none}
.btn-primary{background:#2c5fd6;border-color:#2c5fd6;color:#fff}
.btn-primary:hover{background:#2450b8}
.btn-danger{background:#c0392b;border-color:#c0392b;color:#fff}
.btn-danger:hover{background:#a53125}
.search{display:flex;gap:6px}
.search input{padding:7px 10px;border:1px solid #d3d8e0;border-radius:6px;min-width:220px;font:inherit}
.pager{display:flex;align-items:center;gap:6px;margin-top:14px;flex-wrap:wrap}
.pager a,.pager span{padding:5px 9px;border-radius:5px;border:1px solid #e1e4ea;background:#fff}
.pager .is-current{background:#2c5fd6;border-color:#2c5fd6;color:#fff}
.pager .count{border:0;background:none;color:#6b7280;margin-right:auto}
.sorted-asc::after{content:' \\2191'}
.sorted-desc::after{content:' \\2193'}
.field{margin-bottom:16px;max-width:640px}
.field label{display:block;font-weight:600;margin-bottom:4px}
.field input[type=text],.field input[type=number],.field input[type=date],.field input[type=time],.field input[type=datetime-local],.field select,.field textarea{width:100%;padding:8px 10px;border:1px solid #d3d8e0;border-radius:6px;font:inherit;background:#fff}
.field textarea{min-height:120px;resize:vertical}
.field .help{color:#6b7280;font-size:12px;margin-top:4px}
.field .error{color:#c0392b;font-size:12px;margin-top:4px}
.field.has-error input,.field.has-error select,.field.has-error textarea{border-color:#c0392b}
.readonly{display:block;padding:8px 10px;background:#f2f4f7;border:1px solid #e1e4ea;border-radius:6px;color:#4b5563}
.alert{padding:11px 14px;border-radius:6px;background:#fdeceb;border:1px solid #f3c6c2;color:#8d2a20;margin-bottom:16px;max-width:640px}
.actions{display:flex;gap:8px;align-items:center;margin-top:20px}
.muted{color:#6b7280}
.mono{font-family:ui-monospace,SFMono-Regular,Menlo,monospace;font-size:12px}
.index-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(240px,1fr));gap:12px}
.index-card{background:#fff;border:1px solid #e1e4ea;border-radius:8px;padding:14px}
.index-card h2{margin:0 0 2px;font-size:15px}
";
