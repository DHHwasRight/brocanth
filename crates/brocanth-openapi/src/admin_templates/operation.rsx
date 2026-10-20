use crate::admin::OperationCtx;
use crate::admin_templates::parts;
use crate::admin_templates::parts::Style;

// One operation: what to send and what comes back.
//
// Schemas are named, not drawn. A schema is a tree and this is a preview;
// the name is what a reader can search the JSON for, and the JSON is one
// route away. Drawing the tree here would be re-implementing Redoc inside
// a backoffice tab, badly.

pub fn OperationPage(ctx: &OperationCtx) -> Element {
    <Style />

    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.back_href} dp-boost>API</a>
        <span>/</span>
        <span>{&ctx.operation_id}</span>
    </p>

    <h1 class={parts::heading()}>
        <span class={parts::verb_badge(&ctx.verb)}>{&ctx.verb}</span>
        <span class="ml-2 font-mono text-[19px]">{&ctx.path}</span>
        if ctx.deprecated {
            <span class={parts::deprecated_badge()}>deprecated</span>
        }
    </h1>

    if !ctx.summary.is_empty() {
        <p class="mt-0 mb-1 text-sm text-ink">{&ctx.summary}</p>
    }
    if !ctx.description.is_empty() {
        <p class="mt-0 mb-1 max-w-3xl text-sm text-ink-soft">{&ctx.description}</p>
    }

    <dl class="grid gap-x-6 gap-y-1 mt-4 mb-1 [grid-template-columns:max-content_1fr] text-sm">
        <dt class="text-ink-faint">Operation ID</dt>
        <dd class="m-0 font-mono text-xs text-ink">{&ctx.operation_id}</dd>
        if !ctx.tags.is_empty() {
            <dt class="text-ink-faint">Tags</dt>
            <dd class="m-0 text-ink">{&ctx.tags}</dd>
        }
        // Always shown, including when the answer is "none". An operation
        // with no authentication is the fact a reader most wants to be
        // told, and a missing row reads as a missing feature of the tab.
        <dt class="text-ink-faint">Authentication</dt>
        <dd class="m-0 text-ink">{&ctx.security}</dd>
    </dl>

    <h2 class={parts::subheading()}>Parameters</h2>
    if ctx.parameters.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>None.</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Name</th>
                        <th class={parts::th()}>In</th>
                        <th class={parts::th()}>Type</th>
                        <th class={parts::th()}>Required</th>
                        <th class={parts::th()}>Description</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.parameters {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class={parts::mono()}>{&row.name}</span>
                            </td>
                            <td class={parts::td_muted()}>{&row.location}</td>
                            <td class={parts::td()}>
                                <span class={parts::mono()}>{&row.schema}</span>
                            </td>
                            <td class={parts::td_muted()}>{&row.required}</td>
                            <td class={parts::td_muted()}>{&row.description}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }

    <h2 class={parts::subheading()}>Request body</h2>
    if ctx.request_body.is_empty() {
        <div class={parts::card()}><div class={parts::empty()}>None.</div></div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Media type</th>
                        <th class={parts::th()}>Schema</th>
                        <th class={parts::th()}>Required</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.request_body {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class={parts::mono()}>{&row.media_type}</span>
                            </td>
                            <td class={parts::td()}>
                                <span class={parts::mono()}>{&row.schema}</span>
                            </td>
                            <td class={parts::td_muted()}>{&row.required}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }

    <h2 class={parts::subheading()}>Responses</h2>
    <div class={parts::card()}>
        <table class={parts::table()}>
            <thead>
                <tr>
                    <th class={parts::th()}>Status</th>
                    <th class={parts::th()}>Description</th>
                    <th class={parts::th()}>Content</th>
                </tr>
            </thead>
            <tbody>
                for row in &ctx.responses {
                    <tr class="hover:bg-stage/60">
                        <td class={parts::td()}>
                            <span class={parts::mono()}>{&row.status}</span>
                        </td>
                        <td class={parts::td()}>{&row.description}</td>
                        <td class={parts::td_muted()}>
                            <span class={parts::mono()}>{&row.content}</span>
                        </td>
                    </tr>
                }
            </tbody>
        </table>
    </div>
}
