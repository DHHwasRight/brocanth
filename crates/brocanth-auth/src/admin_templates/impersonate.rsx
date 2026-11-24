use crate::admin::impersonate::ImpersonateCtx;
use crate::admin_templates::chrome::Chrome;
use crate::admin_templates::parts;

// Every account, with a form that starts acting as it.
//
// The form posts to the application's own route, not to the admin: only a
// page can set the session cookie. A reason is required, because the
// session row keeps it for as long as the session lives.

pub fn Impersonate(ctx: &ImpersonateCtx) -> Element {
    <Chrome volatile={ctx.volatile} />
    <h1 class={parts::heading()}>Impersonate</h1>
    <p class="text-sm text-ink-soft mb-4">
        Act as an account to see what it sees. The session records who you are and why, and it cannot change a password, a second factor or a role.
    </p>

    if ctx.rows.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>No account has been created.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Address</th>
                        <th class={parts::th()}>Act as this account</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.rows {
                        <tr>
                            <td class={parts::td()}>{&row.email}</td>
                            <td class={parts::td()}>
                                if ctx.can_act {
                                    <form class="m-0 flex gap-2" method="post" action={&ctx.start}>
                                        <input type="hidden" name="id" value={&row.id} />
                                        <label>Reason <input name="reason" required maxlength="200" /></label>
                                        <button class={parts::danger_button()}>Act as</button>
                                    </form>
                                }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>

        <nav class="flex gap-2.5 mt-4 text-sm" aria-label="Pages">
            if let Some(previous) = &ctx.previous {
                <a class="text-accent no-underline hover:underline" href={previous}>Previous</a>
            }
            <span class="text-ink-soft">{&ctx.summary}</span>
            if let Some(next) = &ctx.next {
                <a class="text-accent no-underline hover:underline" href={next}>Next</a>
            }
        </nav>
    }
}
