use crate::admin::RolesCtx;
use crate::admin_templates::chrome::Chrome;
use crate::admin_templates::parts;

// Every role, what it grants and who holds it.
//
// The buttons are forms that post to the tab itself, with a hidden `do` field
// that names the action. A superuser sees every button: a role holder who
// could edit roles could give itself every permission. The owner of a tab
// sees one: give or take `read` or `edit` on that tab.
//
// Only class names that the panel's stylesheet already carries, so the sheet
// needs no rebuild for this screen.

pub fn Roles(ctx: &RolesCtx) -> Element {
    <Chrome volatile={ctx.volatile} />
    <h1 class={parts::heading()}>Roles</h1>

    if let Some(error) = &ctx.error {
        <div class="mb-4 rounded-card border border-danger bg-white px-3.5 py-3 text-sm text-danger" role="alert">
            {error}
        </div>
    }

    if ctx.roles.is_empty() {
        <div class={parts::card()}>
            <div class={parts::empty()}>No role has been defined.</div>
        </div>
    } else {
        <div class={parts::card()}>
            <table class={parts::table()}>
                <thead>
                    <tr>
                        <th class={parts::th()}>Role</th>
                        <th class={parts::th()}>Permissions</th>
                        <th class={parts::th()}>Members</th>
                    </tr>
                </thead>
                <tbody>
                    for role in &ctx.roles {
                        <tr class="hover:bg-stage/60">
                            <td class={parts::td()}>
                                <span class="font-semibold">{&role.name}</span>
                                if ctx.can_act {
                                    <form class="mt-1" method="post" action={&ctx.action}>
                                        <input type="hidden" name="do" value="delete" />
                                        <input type="hidden" name="role" value={&role.name} />
                                        <button class={parts::danger_button()} type="submit">Delete</button>
                                    </form>
                                }
                            </td>
                            <td class={parts::td_muted()}>
                                for grant in &role.grants {
                                    <span class="flex gap-2.5 items-center mb-1">
                                        <span class="grow font-mono text-xs [overflow-wrap:anywhere]">{&grant.permission}</span>
                                        if grant.revocable {
                                            <form class="m-0" method="post" action={&ctx.action}>
                                                <input type="hidden" name="do" value="revoke" />
                                                <input type="hidden" name="role" value={&role.name} />
                                                <input type="hidden" name="permission" value={&grant.permission} />
                                                <button class={parts::danger_button()} type="submit">Take</button>
                                            </form>
                                        }
                                    </span>
                                }
                                if ctx.can_grant {
                                    <form class="flex gap-2.5 items-end m-0" method="post" action={&ctx.action}>
                                        <input type="hidden" name="do" value="grant" />
                                        <input type="hidden" name="role" value={&role.name} />
                                        <label class="grow">
                                            <span class="block text-xs text-ink-faint">Tab or model</span>
                                            if ctx.grantable.is_empty() {
                                                <input class="w-full border border-line rounded-control px-3 py-1.5 text-sm font-mono" type="text" name="resource" required placeholder="jobs" />
                                            } else {
                                                <select class="w-full border border-line rounded-control px-3 py-1.5 text-sm font-mono" name="resource">
                                                    for resource in &ctx.grantable {
                                                        <option value={resource}>{resource}</option>
                                                    }
                                                </select>
                                            }
                                        </label>
                                        <label>
                                            <span class="block text-xs text-ink-faint">Level</span>
                                            <select class="border border-line rounded-control px-3 py-1.5 text-sm" name="level">
                                                for level in &ctx.levels {
                                                    <option value={level}>{level}</option>
                                                }
                                            </select>
                                        </label>
                                        <button class="bg-white border border-line rounded-control px-3 py-1.5 text-sm text-ink cursor-pointer hover:bg-stage/60" type="submit">Give</button>
                                    </form>
                                }
                            </td>
                            <td class={parts::td()}>
                                for member in &role.members {
                                    <span class="flex gap-2.5 items-center mb-2">
                                        <span class="grow [overflow-wrap:anywhere]">{&member.label}</span>
                                        if ctx.can_act {
                                            <form class="m-0" method="post" action={&ctx.action}>
                                                <input type="hidden" name="do" value="unassign" />
                                                <input type="hidden" name="role" value={&role.name} />
                                                <input type="hidden" name="account" value={&member.account} />
                                                <button class={parts::danger_button()} type="submit">Remove</button>
                                            </form>
                                        }
                                    </span>
                                }
                                if ctx.can_act {
                                    <form class="flex gap-2.5 items-center m-0" method="post" action={&ctx.action}>
                                        <input type="hidden" name="do" value="assign" />
                                        <input type="hidden" name="role" value={&role.name} />
                                        <label class="grow">
                                            <span class="block text-xs text-ink-faint">Give this role to</span>
                                            <input class="w-full border border-line rounded-control px-3 py-1.5 text-sm" type="email" name="email" required placeholder="name@example.com" />
                                        </label>
                                        <button class="bg-white border border-line rounded-control px-3 py-1.5 text-sm text-ink cursor-pointer hover:bg-stage/60" type="submit">Add</button>
                                    </form>
                                }
                            </td>
                        </tr>
                    }
                </tbody>
            </table>
        </div>
    }

    if ctx.can_act {
        <h2 class="text-base font-semibold text-ink mt-4 mb-2">Define a role</h2>
        <div class={parts::card()}>
            <form class="px-3.5 py-3 m-0" method="post" action={&ctx.action}>
                <input type="hidden" name="do" value="define" />
                <label class="block mb-2">
                    <span class="block text-xs text-ink-faint">Name</span>
                    <input class="w-full border border-line rounded-control px-3 py-1.5 text-sm" type="text" name="role" required placeholder="editor" />
                </label>
                <label class="block mb-2">
                    <span class="block text-xs text-ink-faint">Permissions, separated by spaces: a level (read, edit, owner) on a tab or a model. A role with this name gets these permissions instead of its old ones.</span>
                    <input class="w-full border border-line rounded-control px-3 py-1.5 text-sm font-mono" type="text" name="permissions" placeholder="post.read jobs.edit" />
                </label>
                <button class="bg-white border border-line rounded-control px-3 py-1.5 text-sm text-ink cursor-pointer hover:bg-stage/60" type="submit">Save the role</button>
            </form>
        </div>
    }
}
