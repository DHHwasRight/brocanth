use crate::admin::SettingsCtx;
use crate::admin_templates::parts;

// The configuration this process is running.
//
// Two parts, and the first one is not decoration. The sources table says
// which files were found and in what order, so the `[2]` in an origin below
// is a path the reader can open. The layering is the point of the screen: a
// value, the file and line that set it, and every layer it beat.
//
// No form. There is no hot reload, on purpose — a plugin is built from its
// configuration — so a field here would either not take effect or demand a
// lock on every read. Restarting is how a configuration changes, and this is
// how you check what the restart gave you.

pub fn Settings(ctx: &SettingsCtx) -> Element {
    <style>{! crate::admin::STYLE }</style>

    <h1 class={parts::heading()}>Settings</h1>
    <p class="text-sm text-ink-soft mb-5">
        {&ctx.app} · {&ctx.env} · {&ctx.summary}
        if let Some(hidden) = &ctx.hidden {
            <span class="ml-1.5 text-ink-faint">{hidden}</span>
        }
    </p>

    <h2 class="mb-2 text-base font-semibold text-ink">Sources, highest precedence first</h2>
    if ctx.sources.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>
                No configuration file was found, and no environment variable set a value.
                Every plugin is running on its own defaults.
            </div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>#</th>
                        <th class={parts::th()}>Where</th>
                        <th class={parts::th()}>What</th>
                        <th class={parts::th()}>Read as</th>
                    </tr>
                </thead>
                <tbody>
                    for source in &ctx.sources {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td_muted()}>{&source.rank}</td>
                            <td class={parts::td()}>{&source.origin}</td>
                            <td class={parts::td()}>
                                <span class={parts::path()}>{&source.what}</span>
                                // Names only. A variable's value is a key in
                                // the tables below, where it is redacted.
                                if !source.variables.is_empty() {
                                    <span class="block mt-1 font-mono text-xs text-ink-faint [overflow-wrap:anywhere]">
                                        for name in &source.variables {
                                            <span class="mr-1.5">{name}</span>
                                        }
                                    </span>
                                }
                            </td>
                            <td class={parts::td_muted()}>{&source.detail}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }

    if ctx.sections.is_empty() {
        <div class="mt-6">
            <div class={parts::card()}>
                <div class={parts::empty()}>This configuration sets nothing.</div>
            </div>
        </div>
    } else {
        for section in &ctx.sections {
            <h2 class="mt-6 mb-2 text-base font-semibold text-ink">
                <span class="font-mono">[{&section.name}]</span>
            </h2>
            <div class={parts::card()}>
                <table class={parts::table()}>
                    <thead>
                        <tr>
                            <th class={parts::th()}>Key</th>
                            <th class={parts::th()}>Value</th>
                            <th class={parts::th()}>From</th>
                        </tr>
                    </thead>
                    <tbody>
                        for row in &section.rows {
                            <tr class="hover:bg-stage/60">
                                <td class={parts::td()}><span class={parts::path()}>{&row.path}</span></td>
                                <td class={parts::td()}>
                                    if row.redacted {
                                        <span class={parts::redacted()}>{&row.value}</span>
                                    } else {
                                        <span class={parts::literal()}>{&row.value}</span>
                                    }
                                    // Why, so a false positive is a key to
                                    // rename rather than a mystery.
                                    if let Some(reason) = &row.reason {
                                        <span class="block mt-1 text-xs text-ink-faint">{reason}</span>
                                    }
                                    // The layers this value beat. "It is
                                    // 8080, it was 3100 in the YAML" is the
                                    // second question every incident asks.
                                    for layer in &row.history {
                                        <span class={parts::was()}>was {&layer.value} — {&layer.origin}</span>
                                    }
                                </td>
                                <td class={parts::td_muted()}>
                                    <span class={parts::origin()}>{&row.origin}</span>
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </div>
        }
    }

    <p class="mt-6 text-sm text-ink-soft">
        These are the values as found: the files above, and the environment. A plugin's
        own defaults are underneath them and are not on this screen, because this panel
        is built before the plugins claim their sections.
        <span class="block mt-1 font-mono text-xs text-ink-faint">effective_of(app.services()).unwrap().dump()</span>
        <span class="block mt-1">prints the document with the defaults attributed.</span>
    </p>
}
