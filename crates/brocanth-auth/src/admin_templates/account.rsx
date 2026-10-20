use crate::admin::AccountCtx;
use crate::admin_templates::chrome::Chrome;
use crate::admin_templates::parts;

// One account, and the sessions open on it.
//
// The session identifier is shortened on purpose. The whole one is the lookup
// key the cookie carries, and a screenshot of this page taken to put in a
// ticket should not carry one. A prefix tells two rows apart, which is all
// the column is for.
//
// One button, and it ends every session rather than one. Ending one session
// of five is a gesture that looks like a response to a compromise and is not:
// the other four keep working. "Sign out everywhere" is the action an
// operator actually wants, it is what a password change already calls, and
// it is `AuthStore::delete_sessions_for_account`.

pub fn Account(ctx: &AccountCtx) -> Element {
    <Chrome volatile={ctx.volatile} />
    <p class="text-sm mb-4">
        <a class="text-accent no-underline hover:underline" href={&ctx.back} dp-boost>Accounts</a>
    </p>
    <h1 class={parts::heading()}>
        if ctx.has_email {
            <span>{&ctx.email}</span>
        } else {
            <span class="text-ink-faint italic">{&ctx.email}</span>
        }
    </h1>

    <div class="grid gap-2.5 mb-5 [grid-template-columns:repeat(auto-fit,minmax(170px,1fr))]">
        <div>
            <div class={parts::term()}>Identifier</div>
            <div class={parts::definition()}><span class="font-mono text-xs">{&ctx.id}</span></div>
        </div>
        <div>
            <div class={parts::term()}>Address verified</div>
            <div class={parts::definition()}>
                if ctx.verified {
                    <span class={parts::yes()}>yes</span>
                } else {
                    <span class={parts::no()}>no</span>
                }
            </div>
        </div>
        <div>
            <div class={parts::term()}>Password</div>
            <div class={parts::definition()}>
                if ctx.password {
                    <span class={parts::yes()}>set</span>
                } else {
                    <span class={parts::no()}>none — provider sign-in only</span>
                }
            </div>
        </div>
        <div>
            <div class={parts::term()}>Created</div>
            <div class={parts::definition()}>{&ctx.created}</div>
        </div>
    </div>

    <div class="flex gap-2.5 mb-2 items-center">
        <h2 class="text-base font-semibold text-ink m-0 grow">Sessions</h2>
        if ctx.can_act {
            // A form and not a link, because it changes something and a
            // crawler must not be able to fire it. The account is a hidden
            // field rather than a path segment: an account id is opaque and a
            // store may use one with a slash in it.
            if !ctx.sessions.is_empty() {
                <form class="m-0" method="post" action={&ctx.end_href}>
                    <input type="hidden" name="id" value={&ctx.id} />
                    <button class={parts::danger_button()} type="submit">End every session</button>
                </form>
            }
        }
    </div>

    if ctx.sessions.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>Nobody is signed in as this account.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Session</th>
                        <th class={parts::th()}>Privilege</th>
                        <th class={parts::th()}>Proved by</th>
                        <th class={parts::th()}>Started</th>
                        <th class={parts::th()}>Last seen</th>
                        <th class={parts::th()}>Expires</th>
                    </tr>
                </thead>
                <tbody>
                    for row in &ctx.sessions {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-mono text-xs">{&row.id}</span>
                                // A session opened by an operator acting as
                                // somebody else is the one row on this screen
                                // that must never read as an ordinary login.
                                if let Some(operator) = &row.operator {
                                    <span class="block mt-1 max-w-[46ch] text-xs text-danger [overflow-wrap:anywhere]">
                                        impersonated by {operator}
                                    </span>
                                }
                            </td>
                            <td class={parts::td()}>{&row.privilege}</td>
                            <td class={parts::td_muted()}>{&row.factors}</td>
                            <td class={parts::td_muted()}>{&row.started}</td>
                            <td class={parts::td_muted()}>{&row.last_seen}</td>
                            <td class={parts::td_muted()}>{&row.expires}</td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }
}
