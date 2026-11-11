//! Staff in the backoffice: a screen policy over [`Staff`], and a tab that
//! manages roles.
//!
//! # The policy
//!
//! [`StaffPolicy`] reads the principal as a [`Staff`] and asks it for the
//! permission each screen names (see [`crate::staff`] for the table). A
//! superuser passes every check, so an application with superusers and no
//! policy of its own still has a usable backoffice. An anonymous principal,
//! or one of another type, passes none.
//!
//! **Only a superuser changes roles.** A role holder who could edit roles
//! could give itself every permission, so the buttons of the roles tab ask
//! for the superuser flag and not for a permission. This is Django's rule
//! for `auth.change_group` made unconditional.
//!
//! # The tab
//!
//! [`RolesPanel`] lists the roles with their permissions and members, and
//! has four buttons: define a role, delete one, give a role to an account by
//! its address, and take it away. Each button is a POST, and the admin asks
//! the policy before the panel runs.
//!
//! The store can arrive late. A generated project builds the admin when the
//! route table is built, before any plugin has started, so
//! [`RolesPanel::late`] takes a cell that a plugin fills at startup. Until
//! then the tab says the store is not ready.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{Action, Policy, Principal, Request};
use brocanth_admin::view::render;

use crate::identity::{AccountId, EmailAddress};
use crate::staff::Staff;
use crate::storage::{AccountDirectory, Role, StaffStore};

use super::{encode, failed};

/// The roles tab's key, and the subject of its policy questions.
pub const ROLES_KEY: &str = "roles";

/// The permission a backoffice request needs, or `None` for the index,
/// which every member of staff may open.
///
/// ```
/// use brocanth_admin::policy::{Action, Request};
/// use brocanth_auth::admin::permission;
///
/// assert_eq!(permission(Request::new(Action::ViewList, "post")).as_deref(), Some("post.view"));
/// assert_eq!(permission(Request::new(Action::Update, "post")).as_deref(), Some("post.change"));
/// assert_eq!(permission(Request::new(Action::ActOnPanel, "jobs")).as_deref(), Some("jobs.act"));
/// assert_eq!(permission(Request::new(Action::ViewIndex, "")), None);
/// ```
pub fn permission(request: Request<'_>) -> Option<String> {
    let verb = match request.action {
        Action::ViewIndex => return None,
        Action::ViewList | Action::ViewDetail | Action::ViewPanel => "view",
        Action::Create => "add",
        Action::Update => "change",
        Action::Delete => "delete",
        Action::ActOnPanel => "act",
    };
    Some(format!("{}.{verb}", request.model))
}

/// The backoffice's screen policy, over a [`Staff`] principal.
///
/// ```ignore
/// AdminConfig::new().with_policy(StaffPolicy)
/// // and, per request:
/// admin.dispatch(staff.as_ref().map_or_else(Principal::anonymous, |s| Principal::new(s)), ..)
/// ```
#[derive(Clone, Copy, Debug, Default)]
pub struct StaffPolicy;

impl Policy for StaffPolicy {
    fn allows(&self, principal: &Principal<'_>, request: Request<'_>) -> bool {
        let Some(staff) = principal.downcast::<Staff>() else {
            return false;
        };
        if !staff.is_staff() {
            return false;
        }
        if request.action == Action::ActOnPanel && request.model == ROLES_KEY {
            return staff.is_superuser();
        }
        match permission(request) {
            None => true,
            Some(permission) => staff.allows(&permission),
        }
    }
}

/// Where the panel finds its store.
enum Source<S> {
    Now(Arc<S>),
    Late(Arc<OnceLock<Arc<S>>>),
}

/// Roles as a tab: who holds which permissions, and the buttons that change
/// it.
pub struct RolesPanel<S: StaffStore + AccountDirectory> {
    source: Source<S>,
}

impl<S: StaffStore + AccountDirectory> RolesPanel<S> {
    /// The tab over a store the application already holds.
    pub fn new(store: Arc<S>) -> Self {
        Self {
            source: Source::Now(store),
        }
    }

    /// The tab over a store that a plugin puts in `cell` at startup.
    pub fn late(cell: Arc<OnceLock<Arc<S>>>) -> Self {
        Self {
            source: Source::Late(cell),
        }
    }

    fn store(&self) -> Result<&Arc<S>, AdminError> {
        match &self.source {
            Source::Now(store) => Ok(store),
            Source::Late(cell) => cell.get().ok_or_else(|| {
                AdminError::Render(rsx::Error::custom(
                    "the roles tab has no store yet: the application has not started\n\
                     help: the plugin that owns the store fills the cell in before_start",
                ))
            }),
        }
    }
}

#[async_trait]
impl<S: StaffStore + AccountDirectory> Panel for RolesPanel<S> {
    fn key(&self) -> &str {
        ROLES_KEY
    }

    fn label(&self) -> String {
        "Roles".to_string()
    }

    async fn render(&self, r: PanelRequest<'_>) -> Result<Option<PanelPage>, AdminError> {
        if !r.path.is_empty() {
            return Ok(None);
        }
        if r.post {
            return self.act(&r).await.map(Some);
        }
        self.list(&r).await.map(Some)
    }
}

impl<S: StaffStore + AccountDirectory> RolesPanel<S> {
    async fn list(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let store = self.store()?;
        let mut rows = Vec::new();
        for role in store.roles().await.map_err(failed)? {
            let mut members = Vec::new();
            for id in store.members(&role.name).await.map_err(failed)? {
                let label = match store.account(&id).await.map_err(failed)? {
                    Some(account) => account
                        .email
                        .map_or_else(|| id.to_string(), |e| e.to_string()),
                    None => id.to_string(),
                };
                members.push(MemberRow {
                    account: id.to_string(),
                    label,
                });
            }
            rows.push(RoleRow {
                permissions: role
                    .permissions
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" "),
                name: role.name,
                members,
            });
        }
        let ctx = RolesCtx {
            volatile: !store.is_durable(),
            can_act: r.can_act,
            action: r.url(""),
            error: r.param("error"),
            roles: rows,
        };
        let view = |ctx: &RolesCtx, w: &mut rsx::Writer<'_>| {
            use rsx::Template as _;
            crate::admin_templates::roles::Roles { ctx }.render(w)
        };
        Ok(PanelPage::body("Roles", render(&view, &ctx)?))
    }

    /// Every button. The form's `do` field says which.
    async fn act(&self, r: &PanelRequest<'_>) -> Result<PanelPage, AdminError> {
        let store = self.store()?;
        let field = |name: &str| r.submission.get(name).unwrap_or("").trim().to_string();
        let back = |error: Option<&str>| match error {
            None => PanelPage::redirect(r.url("")),
            Some(message) => {
                PanelPage::redirect(format!("{}?error={}", r.url(""), encode(message)))
            }
        };
        let name = field("role");
        match field("do").as_str() {
            "define" => {
                if !valid_name(&name) {
                    return Ok(back(Some(
                        "a role name is letters, digits, dots, dashes and underscores",
                    )));
                }
                let permissions = field("permissions");
                let role = Role::new(
                    name,
                    permissions
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter(|p| !p.is_empty()),
                );
                store.put_role(&role).await.map_err(failed)?;
            }
            "delete" => {
                store.delete_role(&name).await.map_err(failed)?;
            }
            "assign" => {
                let Ok(address) = EmailAddress::parse(&field("email")) else {
                    return Ok(back(Some("that is not an email address")));
                };
                let Some(account) = store.account_by_email(&address).await.map_err(failed)? else {
                    return Ok(back(Some("no account has that address")));
                };
                if !store
                    .assign_role(&account.id, &name)
                    .await
                    .map_err(failed)?
                {
                    return Ok(back(Some("no role has that name")));
                }
            }
            "unassign" => {
                let account = AccountId::new(field("account"));
                store.unassign_role(&account, &name).await.map_err(failed)?;
            }
            _ => return Ok(back(Some("the form named no action"))),
        }
        Ok(back(None))
    }
}

/// A role name fits in a permission string and a URL without escaping.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

// ---------------------------------------------------------------------------
// What the template is given
// ---------------------------------------------------------------------------

/// The roles tab.
pub struct RolesCtx {
    /// Whether the roles go when the process does.
    pub volatile: bool,
    /// Whether to draw the buttons.
    pub can_act: bool,
    /// Where every form posts.
    pub action: String,
    /// What the last button got wrong, from the redirect.
    pub error: Option<String>,
    /// Every role, by name.
    pub roles: Vec<RoleRow>,
}

/// One role.
pub struct RoleRow {
    /// Its name.
    pub name: String,
    /// Its permissions, space separated, sorted.
    pub permissions: String,
    /// Who holds it.
    pub members: Vec<MemberRow>,
}

/// One holder of a role.
pub struct MemberRow {
    /// The account id, for the button that takes the role away.
    pub account: String,
    /// The address, or the id when there is none.
    pub label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_role_name_is_a_word() {
        assert!(valid_name("editor"));
        assert!(valid_name("support.tier-2_eu"));
        assert!(!valid_name(""));
        assert!(!valid_name("two words"));
        assert!(!valid_name("a/b"));
    }
}
