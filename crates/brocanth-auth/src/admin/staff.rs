//! Staff in the backoffice: a screen policy over [`Staff`], and a tab that
//! manages roles.
//!
//! # The policy
//!
//! [`StaffPolicy`] reads the principal as a [`Staff`] and asks it for the
//! level each screen needs on its RBAC resource: `<resource>.read` to look,
//! `<resource>.edit` to press a button or write (see [`crate::staff`] for
//! the table). A superuser passes every check, so an application with
//! superusers and no policy of its own still has a usable backoffice. An
//! anonymous principal, or one of another type, passes none.
//!
//! **Only a superuser changes roles**, with one exception inside one tab.
//! A role holder who could edit roles could give itself every permission,
//! so defining a role, deleting one, and giving or taking a role ask for
//! the superuser flag. The exception: the owner of a resource
//! (`jobs.owner`) gives or takes `jobs.read` and `jobs.edit` on a role, and
//! nothing else. Only a superuser gives `owner`. So an owner cannot raise
//! itself or reach past its own tab (`docs/module-capabilities.md` §5.2).
//!
//! # The tab
//!
//! [`RolesPanel`] lists the roles with their permissions and members. A
//! superuser has five buttons: define a role, delete one, give a role to an
//! account by its address, take it away, and give or take a level on a
//! resource. An owner has the last one, for its own resources. Each button
//! is a POST; the admin asks the policy before the panel runs, and the
//! panel checks the principal for the button pressed.
//!
//! The store can arrive late. A generated project builds the admin when the
//! route table is built, before any plugin has started, so
//! [`RolesPanel::late`] takes a [`Late`] cell that the admin fills from the
//! first request that carries the store. Until then the tab says the store
//! is not ready.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{Action, Level, Policy, Principal, Request};
use brocanth_admin::view::render;

use crate::identity::{AccountId, EmailAddress};
use crate::staff::Staff;
use crate::storage::{AccountDirectory, Role, StaffStore};

use super::failed;

/// The roles tab's key, and the subject of its policy questions.
pub const ROLES_KEY: &str = "roles";

/// The permission a backoffice request needs: a level on the request's
/// RBAC resource, or `None` for the index, which every member of staff may
/// open.
///
/// ```
/// use brocanth_admin::policy::{Action, Request};
/// use brocanth_auth::admin::permission;
///
/// assert_eq!(permission(Request::new(Action::ViewList, "post")).as_deref(), Some("post.read"));
/// assert_eq!(permission(Request::new(Action::Update, "post")).as_deref(), Some("post.edit"));
/// assert_eq!(permission(Request::new(Action::ActOnPanel, "jobs")).as_deref(), Some("jobs.edit"));
/// assert_eq!(permission(Request::new(Action::ViewIndex, "")), None);
/// ```
pub fn permission(request: Request<'_>) -> Option<String> {
    request.action.level().map(|level| level.on(request.model))
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
        if request.model == ROLES_KEY {
            // An owner of any resource opens the roles tab and presses its
            // grant buttons; the panel narrows each button to what this
            // person may give. Everything else in it is a superuser's.
            let owner = !staff.owned().is_empty();
            return match request.action {
                Action::ViewPanel => owner || staff.allows(&Level::Read.on(ROLES_KEY)),
                Action::ActOnPanel => owner || staff.is_superuser(),
                _ => staff.is_superuser(),
            };
        }
        match permission(request) {
            None => true,
            Some(permission) => staff.allows(&permission),
        }
    }
}

/// Roles as a tab: who holds which permissions, and the buttons that change
/// it.
pub struct RolesPanel<S: StaffStore + AccountDirectory> {
    store: Late<Arc<S>>,
}

impl<S: StaffStore + AccountDirectory> RolesPanel<S> {
    /// The tab over a store the application already holds.
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store: Late::ready(store),
        }
    }

    /// The tab over a store that arrives later, in `cell`: the admin fills
    /// it from the first request that carries the auth services.
    pub fn late(cell: Late<Arc<S>>) -> Self {
        Self { store: cell }
    }

    fn store(&self) -> Result<&Arc<S>, AdminError> {
        self.store.require("accounts store")
    }
}

/// What the person on a request may change in the roles tab.
#[derive(Debug, Default)]
struct Grantor {
    /// Defines, deletes, gives and takes roles, and gives any level.
    superuser: bool,
    /// The resources it owns: it gives `read` and `edit` on each.
    owns: Vec<String>,
}

impl Grantor {
    fn of(principal: &Principal<'_>) -> Self {
        match principal.downcast::<Staff>() {
            Some(staff) if staff.is_staff() => Grantor {
                superuser: staff.is_superuser(),
                owns: staff.owned().into_iter().map(str::to_string).collect(),
            },
            _ => Grantor::default(),
        }
    }

    /// Whether it may give or take `level` on `resource`. Only a superuser
    /// gives `owner`; an owner gives `read` and `edit` on its own resource.
    fn may_grant(&self, resource: &str, level: Level) -> bool {
        self.superuser || (level != Level::Owner && self.owns.iter().any(|r| r == resource))
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
        let grantor = Grantor::of(&r.principal);
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
                grants: role
                    .permissions
                    .iter()
                    .map(|permission| GrantRow {
                        revocable: r.can_act
                            && split(permission).is_some_and(|(resource, level)| {
                                grantor.may_grant(resource, level)
                            }),
                        permission: permission.clone(),
                    })
                    .collect(),
                name: role.name,
                members,
            });
        }
        let ctx = RolesCtx {
            volatile: !store.is_durable(),
            can_act: r.can_act && grantor.superuser,
            can_grant: r.can_act && (grantor.superuser || !grantor.owns.is_empty()),
            grantable: grantor.owns.clone(),
            levels: Level::ALL
                .into_iter()
                .filter(|level| grantor.superuser || *level != Level::Owner)
                .map(|level| level.as_str().to_string())
                .collect(),
            action: r.url(""),
            error: r
                .param("error")
                .and_then(|code| Refusal::from_code(&code))
                .map(|refusal| refusal.message().to_string()),
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
        let back = |refusal: Option<Refusal>| match refusal {
            None => PanelPage::redirect(r.url("")),
            Some(refusal) => PanelPage::redirect(format!("{}?error={}", r.url(""), refusal.code())),
        };
        let name = field("role");
        let grantor = Grantor::of(&r.principal);
        let action = field("do");
        if matches!(action.as_str(), "grant" | "revoke") {
            return self.grant(r, &grantor, &name, action == "grant").await;
        }
        // Everything but a grant is a superuser's: the policy let an owner
        // through to the tab, and this is where the tab narrows it.
        if !grantor.superuser {
            return Ok(back(Some(Refusal::NotYours)));
        }
        match action.as_str() {
            "define" => {
                if !valid_name(&name) {
                    return Ok(back(Some(Refusal::BadName)));
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
                    return Ok(back(Some(Refusal::NotAnAddress)));
                };
                let Some(account) = store.account_by_email(&address).await.map_err(failed)? else {
                    return Ok(back(Some(Refusal::NoAccount)));
                };
                if !store
                    .assign_role(&account.id, &name)
                    .await
                    .map_err(failed)?
                {
                    return Ok(back(Some(Refusal::NoRole)));
                }
            }
            "unassign" => {
                let account = AccountId::new(field("account"));
                store.unassign_role(&account, &name).await.map_err(failed)?;
            }
            _ => return Ok(back(Some(Refusal::NoAction))),
        }
        Ok(back(None))
    }

    /// Gives or takes one level on one resource to or from a role.
    async fn grant(
        &self,
        r: &PanelRequest<'_>,
        grantor: &Grantor,
        role: &str,
        give: bool,
    ) -> Result<PanelPage, AdminError> {
        let store = self.store()?;
        let back = |refusal: Option<Refusal>| match refusal {
            None => PanelPage::redirect(r.url("")),
            Some(refusal) => PanelPage::redirect(format!("{}?error={}", r.url(""), refusal.code())),
        };
        let field = |name: &str| r.submission.get(name).unwrap_or("").trim().to_string();
        // The take button names the permission; the give form names the
        // resource and the level.
        let permission = field("permission");
        let (resource, level) = if permission.is_empty() {
            (field("resource"), Level::parse(&field("level")))
        } else {
            match split(&permission) {
                Some((resource, level)) => (resource.to_string(), Some(level)),
                None => (String::new(), None),
            }
        };
        let Some(level) = level else {
            return Ok(back(Some(Refusal::NoLevel)));
        };
        if !valid_resource(&resource) {
            return Ok(back(Some(Refusal::BadResource)));
        }
        if !grantor.may_grant(&resource, level) {
            return Ok(back(Some(Refusal::NotYours)));
        }
        let roles = store.roles().await.map_err(failed)?;
        let Some(mut found) = roles.into_iter().find(|candidate| candidate.name == role) else {
            return Ok(back(Some(Refusal::NoRole)));
        };
        let permission = level.on(&resource);
        if give {
            found.permissions.insert(permission);
        } else {
            found.permissions.remove(&permission);
        }
        store.put_role(&found).await.map_err(failed)?;
        Ok(back(None))
    }
}

/// `jobs.edit` as `("jobs", Level::Edit)`, or `None` for a permission that
/// is not a level on a resource, such as `reports.export`.
fn split(permission: &str) -> Option<(&str, Level)> {
    let (resource, level) = permission.rsplit_once('.')?;
    Some((resource, Level::parse(level)?))
}

/// A resource is a tab's or a model's key: lower case letters, digits, `-`
/// and `_`, so `<resource>.<level>` reads one way.
fn valid_resource(resource: &str) -> bool {
    resource.starts_with(|c: char| c.is_ascii_lowercase())
        && resource.len() <= 64
        && resource
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
}

/// Why a button did nothing, carried to the tab by the redirect.
///
/// The redirect carries a code, not the message. A message in the query
/// string is text that any link can set, and the tab would show it to a
/// member of staff as if the admin had written it: "your session expired,
/// sign in at …". With a code, the tab shows only its own sentences, and a
/// code it does not know shows nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refusal {
    BadName,
    NotAnAddress,
    NoAccount,
    NoRole,
    NoAction,
    NoLevel,
    BadResource,
    NotYours,
}

impl Refusal {
    const ALL: [Refusal; 8] = [
        Refusal::BadName,
        Refusal::NotAnAddress,
        Refusal::NoAccount,
        Refusal::NoRole,
        Refusal::NoAction,
        Refusal::NoLevel,
        Refusal::BadResource,
        Refusal::NotYours,
    ];

    /// What the redirect carries in `error=`.
    fn code(self) -> &'static str {
        match self {
            Refusal::BadName => "bad-name",
            Refusal::NotAnAddress => "not-an-address",
            Refusal::NoAccount => "no-account",
            Refusal::NoRole => "no-role",
            Refusal::NoAction => "no-action",
            Refusal::NoLevel => "no-level",
            Refusal::BadResource => "bad-resource",
            Refusal::NotYours => "not-yours",
        }
    }

    /// The refusal a code names, or `None` for a code the tab did not write.
    fn from_code(code: &str) -> Option<Refusal> {
        Refusal::ALL.into_iter().find(|r| r.code() == code)
    }

    /// What the tab shows.
    fn message(self) -> &'static str {
        match self {
            Refusal::BadName => "a role name is letters, digits, dots, dashes and underscores",
            Refusal::NotAnAddress => "that is not an email address",
            Refusal::NoAccount => "no account has that address",
            Refusal::NoRole => "no role has that name",
            Refusal::NoAction => "the form named no action",
            Refusal::NoLevel => "a level is read, edit or owner",
            Refusal::BadResource => {
                "a resource is a tab's or a model's key: lower case letters, digits, dashes and underscores"
            }
            Refusal::NotYours => {
                "only a superuser does that; the owner of a tab gives read or edit on that tab only"
            }
        }
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
    /// Whether to draw a superuser's buttons: define, delete, give and take
    /// a role.
    pub can_act: bool,
    /// Whether to draw the form that gives a level on a resource.
    pub can_grant: bool,
    /// The resources this person may give levels on, for an owner. Empty
    /// for a superuser, who types any resource.
    pub grantable: Vec<String>,
    /// The levels this person may give: `owner` only for a superuser.
    pub levels: Vec<String>,
    /// Where every form posts.
    pub action: String,
    /// What the last button got wrong: the tab's own sentence for the code
    /// the redirect carried. `None` for no code, or one the tab did not
    /// write.
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
    /// Its permissions, one by one, with whether this person may take each
    /// away.
    pub grants: Vec<GrantRow>,
    /// Who holds it.
    pub members: Vec<MemberRow>,
}

/// One permission of a role.
pub struct GrantRow {
    /// `jobs.edit`.
    pub permission: String,
    /// Whether this person may take it away.
    pub revocable: bool,
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
    fn only_a_superuser_gives_owner_and_an_owner_stays_in_its_tab() {
        let owner = Grantor {
            superuser: false,
            owns: vec!["jobs".to_string()],
        };
        assert!(owner.may_grant("jobs", Level::Read));
        assert!(owner.may_grant("jobs", Level::Edit));
        assert!(
            !owner.may_grant("jobs", Level::Owner),
            "only a superuser gives owner"
        );
        assert!(!owner.may_grant("cache", Level::Read), "not its tab");
        let superuser = Grantor {
            superuser: true,
            owns: Vec::new(),
        };
        assert!(superuser.may_grant("cache", Level::Owner));
        assert!(!Grantor::default().may_grant("jobs", Level::Read));
        assert_eq!(split("jobs.edit"), Some(("jobs", Level::Edit)));
        assert_eq!(split("reports.export"), None);
        assert!(valid_resource("oauth-connections"));
        assert!(!valid_resource("Jobs") && !valid_resource("a.b") && !valid_resource(""));
    }

    #[test]
    fn a_role_name_is_a_word() {
        assert!(valid_name("editor"));
        assert!(valid_name("support.tier-2_eu"));
        assert!(!valid_name(""));
        assert!(!valid_name("two words"));
        assert!(!valid_name("a/b"));
    }
}
