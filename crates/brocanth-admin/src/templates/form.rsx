use crate::form::{Field, Widget};
use crate::view::FormCtx;

/// The create and the edit screen, which are the same screen.
///
/// Every input is filled from `field.value`, which holds what was submitted
/// after a rejected write — never a value read back from the database. That is
/// the whole reason this screen takes a built `Form` rather than a row.
pub fn FormPage(ctx: &FormCtx) -> Element {
    <p class="crumbs">
        <a href={&ctx.chrome.base_path}>{&ctx.chrome.title}</a> <span> / </span> <a href={&ctx.model.href}>{&ctx.model.plural}</a> <span> / </span> {action_word(ctx)}
    </p>
    <h1>{heading(ctx)}</h1>

    for message in &ctx.form.errors {
        <div class="alert">{message}</div>
    }

    <form method="post" action={&ctx.action}>
        for field in &ctx.form.fields {
            <FormField field={field} />
        }
        <div class="actions">
            <button class="btn btn-primary" type="submit">{&ctx.submit_label}</button>
            <a class="btn" href={&ctx.cancel_href}>Cancel</a>
        </div>
    </form>
}

/// One labelled input, with its errors under it.
pub fn FormField(field: &Field) -> Element {
    <div class={field_class(field)}>
        <label for={&field.name}>{&field.label}{required_mark(field)}</label>
        match field.widget {
            Widget::Textarea => {
                <textarea id={&field.name} name={&field.name} rows="6">{&field.value}</textarea>
            }
            Widget::Json => {
                <textarea id={&field.name} name={&field.name} rows="6" spellcheck="false" class="mono">{&field.value}</textarea>
            }
            Widget::Checkbox => {
                <input type="checkbox" id={&field.name} name={&field.name} value="on" checked?={field.checked} />
            }
            Widget::Select => <Options field={field} />,
            Widget::ForeignKey => <Options field={field} />,
            Widget::ReadOnly => {
                <output class="readonly" id={&field.name}>{readonly_text(field)}</output>
            }
            Widget::Hidden => {
                <input type="hidden" name={&field.name} value={&field.value} />
            }
            _ => {
                <input
                    type={field.widget.input_type()}
                    id={&field.name}
                    name={&field.name}
                    value={&field.value}
                    required?={field.required}
                />
            }
        }
        if !field.help.is_empty() {
            <p class="help">{&field.help}</p>
        }
        for message in &field.errors {
            <p class="error">{message}</p>
        }
    </div>
}

/// A `<select>`, used by both an enum column and a foreign key.
///
/// A foreign key's options carry the target record's label, which is the one
/// thing an admin must never render as an integer.
pub fn Options(field: &Field) -> Element {
    <select id={&field.name} name={&field.name} required?={field.required}>
        if !field.required {
            <option value="">—</option>
        }
        for choice in &field.choices {
            <option value={&choice.value} selected?={choice.value == field.value}>{&choice.label}</option>
        }
    </select>
}

fn field_class(field: &Field) -> &'static str {
    if field.has_errors() {
        "field has-error"
    } else {
        "field"
    }
}

fn required_mark(field: &Field) -> &'static str {
    if field.required {
        " *"
    } else {
        ""
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
