use crate::view::DetailCtx;

/// One record, field by field.
pub fn Detail(ctx: &DetailCtx) -> Element {
    <p class="crumbs">
        <a href={&ctx.chrome.base_path}>{&ctx.chrome.title}</a> <span> / </span> <a href={&ctx.model.href}>{&ctx.model.plural}</a> <span> / </span> {&ctx.label}
    </p>
    <div class="bar">
        <h1>{&ctx.label}</h1>
        <span class="bar-fill"></span>
        if let Some(href) = &ctx.edit_href {
            <a class="btn btn-primary" href={href}>Edit</a>
        }
        if let Some(href) = &ctx.delete_href {
            <a class="btn btn-danger" href={href}>Delete</a>
        }
    </div>
    <div class="card">
        <table>
            <tbody>
                for row in &ctx.rows {
                    <tr>
                        <th scope="row">{&row.label}</th>
                        <td>
                            if let Some(href) = &row.href {
                                <a href={href}>{&row.text}</a>
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
