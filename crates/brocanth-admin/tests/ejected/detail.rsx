// Ejected from brocanth-admin 0.1.0 by `brocanth admin eject detail`.
//
// Yours now: an upgrade of brocanth-admin does not change this file.
// After one, `brocanth admin diff` compares it with the shipped
// `detail.rsx`. Utility classes that the shipped stylesheet does not
// contain have no effect: add a stylesheet of your own for those
// (`AdminConfig::stylesheet`).

use brocanth_admin::parts;
use brocanth_admin::view::DetailCtx;

/// One record, field by field.
pub fn Detail(ctx: &DetailCtx) -> Element {
    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.chrome.base_path} dp-boost>{&ctx.chrome.title}</a>
        <span>/</span>
        <a class={parts::action()} href={&ctx.model.href} dp-boost>{&ctx.model.plural}</a>
        <span>/</span>
        <span class="text-ink-soft">{&ctx.label}</span>
    </p>
    <div class={parts::bar()}>
        <h1 class={parts::heading()}>{&ctx.label}</h1>
        <span class="flex-1"></span>
        if let Some(href) = &ctx.edit_href {
            <a class={parts::button_primary()} href={href} dp-boost>Edit</a>
        }
        if let Some(href) = &ctx.delete_href {
            <a class={parts::button_danger()} href={href} dp-boost>Delete</a>
        }
    </div>
    <div class={parts::card()}>
        <table class={parts::table()}>
            <tbody>
                for row in &ctx.rows {
                    <tr>
                        // The label column is a `th`, because it names the
                        // row rather than holding data.
                        <th scope="row"
                            class="w-52 text-left px-3.5 py-2.5 border-b border-line align-top font-medium text-ink-soft">
                            {&row.label}
                        </th>
                        <td class={parts::td()}>
                            if let Some(href) = &row.href {
                                <a class={parts::action()} href={href} dp-boost>{&row.text}</a>
                            } else {
                                <span>{&row.text}</span>
                            }
                        </td>
                    </tr>
                }
            </tbody>
        </table>
    </div>
}
