use crate::form::{Field, Widget};
use crate::templates::parts;
use crate::view::FormCtx;

/// The create and the edit screen, which are the same screen.
///
/// Every input is filled from `field.value`, which holds what was submitted
/// after a rejected write — never a value read back from the database. That is
/// the whole reason this screen takes a built `Form` rather than a row.
pub fn FormPage(ctx: &FormCtx) -> Element {
    <p class={parts::crumbs()}>
        <a class={parts::action()} href={&ctx.chrome.base_path} dp-boost>{&ctx.chrome.title}</a>
        <span>/</span>
        <a class={parts::action()} href={&ctx.model.href} dp-boost>{&ctx.model.plural}</a>
        <span>/</span>
        <span>{action_word(ctx)}</span>
    </p>
    <h1 class={parts::heading()}>{heading(ctx)}</h1>

    for message in &ctx.form.errors {
        <div class="mt-4 max-w-2xl rounded-control border border-danger/30 bg-danger/5 px-3.5 py-2.5 text-sm text-danger">
            {message}
        </div>
    }

    // No `dp-` anything on this form. dp-swap enhances navigation and
    // leaves mutating verbs to the server, so this posts and reloads.
    <form class="mt-5" method="post" action={&ctx.action}>
        for field in &ctx.form.fields {
            <FormField field={field} />
        }
        <div class="flex items-center gap-2 mt-6">
            <button class={parts::button_primary()} type="submit">{&ctx.submit_label}</button>
            <a class={parts::button()} href={&ctx.cancel_href} dp-boost>Cancel</a>
        </div>
    </form>
}

/// One labelled input, with its errors under it.
pub fn FormField(field: &Field) -> Element {
    <div class="mb-4 max-w-2xl">
        <label class="block mb-1 text-sm font-medium text-ink" for={&field.name}>
            {&field.label}
            if field.required {
                <span class="text-danger" aria-hidden="true"> *</span>
            }
        </label>
        match field.widget {
            Widget::Textarea => {
                <textarea class={control_class(field)} id={&field.name} name={&field.name} rows="6">{&field.value}</textarea>
            }
            Widget::Json => {
                <textarea class={json_class(field)} id={&field.name} name={&field.name} rows="6" spellcheck="false">{&field.value}</textarea>
            }
            Widget::Checkbox => {
                <input class="size-4 rounded border-line text-accent accent-[var(--color-accent)]" type="checkbox" id={&field.name} name={&field.name} value="on" checked?={field.checked} />
            }
            Widget::Select => <Options field={field} />,
            Widget::ForeignKey => <Options field={field} />,
            Widget::ReadOnly => {
                <output class="block rounded-control border border-line bg-stage px-2.5 py-2 text-sm text-ink-soft" id={&field.name}>{readonly_text(field)}</output>
            }
            Widget::Hidden => {
                <input type="hidden" name={&field.name} value={&field.value} />
            }
            _ => {
                <input
                    class={control_class(field)}
                    type={field.widget.input_type()}
                    id={&field.name}
                    name={&field.name}
                    value={&field.value}
                    required?={field.required}
                />
            }
        }
        if !field.help.is_empty() {
            <p class="mt-1 text-xs text-ink-faint">{&field.help}</p>
        }
        for message in &field.errors {
            <p class="mt-1 text-xs text-danger">{message}</p>
        }
    </div>
}

/// A `<select>`, used by both an enum column and a foreign key.
///
/// A foreign key's options carry the target record's label, which is the one
/// thing an admin must never render as an integer.
pub fn Options(field: &Field) -> Element {
    <select class={control_class(field)} id={&field.name} name={&field.name} required?={field.required}>
        if !field.required {
            <option value="">—</option>
        }
        for choice in &field.choices {
            <option value={&choice.value} selected?={choice.value == field.value}>{&choice.label}</option>
        }
    </select>
}

fn control_class(field: &Field) -> &'static str {
    if field.has_errors() {
        parts::control_error()
    } else {
        parts::control()
    }
}

/// The same control, in the typeface JSON has to be read in.
fn json_class(field: &Field) -> &'static str {
    if field.has_errors() {
        "w-full rounded-control border border-danger bg-white px-2.5 py-2 font-mono text-xs \
         text-ink focus:outline-none focus:ring-2 focus:ring-danger/20"
    } else {
        "w-full rounded-control border border-line bg-white px-2.5 py-2 font-mono text-xs \
         text-ink focus:outline-none focus:border-accent focus:ring-2 focus:ring-accent/20"
    }
}

fn readonly_text(field: &Field) -> &str {
    if field.value.is_empty() {
        "—"
    } else {
        &field.value
    }
}

fn action_word(ctx: &FormCtx) -> &'static str {
    if ctx.is_new {
        "Add"
    } else {
        "Edit"
    }
}

fn heading(ctx: &FormCtx) -> String {
    if ctx.is_new {
        format!("Add {}", ctx.model.name.to_lowercase())
    } else {
        format!("Edit {}", ctx.model.name.to_lowercase())
    }
}
