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
//!
//! The tab reads and changes through [`RolesData`] (docs/backoffice.md
//! §7.6): in this process over the store, or over the backoffice wire
//! ([`RemoteRoles`], answered by [`RolesServe`]). The wire's data side
//! checks the rule above from the call's claim: reading needs `roles.read`
//! or to own a resource, a grant needs the superuser flag or the owner of
//! its resource, and every other button the superuser flag.

use std::sync::Arc;

use async_trait::async_trait;
use brocanth_admin::admin::AdminError;
use brocanth_admin::late::Late;
use brocanth_admin::panel::remote::{Grants, PanelServe, PanelWire};
use brocanth_admin::panel::{Panel, PanelPage, PanelRequest};
use brocanth_admin::policy::{Action, Level, Policy, Principal, Request};
use brocanth_admin::view::render;
use serde::{Deserialize, Serialize};

use crate::identity::{AccountId, EmailAddress};
use crate::staff::Staff;
use crate::storage::{AccountDirectory, Role, StaffStore};

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

/// What the roles tab reads and changes (docs/backoffice.md §7.6, P1).
///
/// In this process it runs on the store; [`RemoteRoles`] sends it over the
/// backoffice wire, and [`RolesServe`] answers it there. The tab narrows
/// each button to what the person may give; on the wire the data side
/// checks the same rule from the claim ([`RolesCall::allows`]).
#[async_trait]
pub trait RolesData: Send + Sync + 'static {
    /// Every role, with its permissions and its members.
    ///
    /// # Errors
    ///
    /// The failure as text, with a `help:` line.
    async fn read(&self, who: &Principal<'_>) -> Result<RolesView, String>;

    /// Runs one button.
    ///
    /// # Errors
    ///
    /// The store's failure as text. A refusal the tab explains (no such
    /// role, no such account) is a [`RolesOutcome`], not an error.
    async fn change(
        &self,
        who: &Principal<'_>,
        change: RolesChange,
    ) -> Result<RolesOutcome, String>;
}

/// The roles, as the tab shows them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RolesView {
    /// Whether the roles stay when the process stops.
    pub durable: bool,
    /// Every role, by name.
    pub roles: Vec<RoleView>,
}

/// One role.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleView {
    /// Its name.
    pub name: String,
    /// Its permissions, sorted.
    pub permissions: Vec<String>,
    /// Who holds it: the account id and its address, or the id.
    pub members: Vec<(String, String)>,
}

/// One button of the roles tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolesChange {
    /// Defines `role` with `permissions`, or redefines it. A superuser's.
    Define {
        /// The role.
        role: String,
        /// Its permission strings.
        permissions: Vec<String>,
    },
    /// Deletes `role`. A superuser's.
    Delete {
        /// The role.
        role: String,
    },
    /// Gives `role` to the account at `email`. A superuser's.
    Assign {
        /// The role.
        role: String,
        /// The account's address.
        email: String,
    },
    /// Takes `role` from `account`. A superuser's.
    Unassign {
        /// The role.
        role: String,
        /// The account id.
        account: String,
    },
    /// Gives (`give`) or takes `level` on `resource` to or from `role`. A
    /// superuser's, or the owner's of `resource` for `read` and `edit`.
    Grant {
        /// The role.
        role: String,
        /// The resource: a tab's or a model's key.
        resource: String,
        /// `read`, `edit` or `owner`.
        level: String,
        /// Give, or take.
        give: bool,
    },
}

impl RolesChange {
    /// Whether `grants` may press this button: the rule of [`Grantor`].
    fn allows(&self, grants: &Grants) -> bool {
        match self {
            RolesChange::Grant {
                resource, level, ..
            } => {
                grants.superuser
                    || Level::parse(level).is_some_and(|level| {
                        level != Level::Owner && grants.holds(resource, Level::Owner)
                    })
            }
            _ => grants.superuser,
        }
    }
}

/// What a button did: done, or the code of the tab's own sentence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolesOutcome {
    /// It changed the roles.
    Done,
    /// It changed nothing; the code says why (`no-role`).
    Refused(String),
}

impl From<Option<Refusal>> for RolesOutcome {
    fn from(refusal: Option<Refusal>) -> Self {
        match refusal {
            None => RolesOutcome::Done,
            Some(refusal) => RolesOutcome::Refused(refusal.code().to_string()),
        }
    }
}

/// One call of [`RolesData`], as the wire carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolesCall {
    /// [`RolesData::read`]. Needs `roles.read`, or to own any resource.
    Read,
    /// [`RolesData::change`]. A superuser's, but for a grant an owner may
    /// make.
    Change(RolesChange),
}

impl RolesCall {
    /// The level the call needs on `roles`, as the refusal names it.
    pub fn level(&self) -> Level {
        match self {
            RolesCall::Read => Level::Read,
            RolesCall::Change(_) => Level::Edit,
        }
    }

    /// Whether `grants` may make this call: the rule [`StaffPolicy`] and
    /// the tab apply in this process.
    pub fn allows(&self, grants: &Grants) -> bool {
        match self {
            RolesCall::Read => {
                grants.holds(ROLES_KEY, Level::Read) || grants.owned().next().is_some()
            }
            RolesCall::Change(change) => change.allows(grants),
        }
    }
}

/// What a [`RolesCall`] answers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RolesAnswer {
    /// To `Read`.
    View(RolesView),
    /// To `Change`.
    Outcome(RolesOutcome),
}

/// The store in this process: empty until the first request that carries
/// it.
struct Stored<S>(Late<Arc<S>>);

impl<S: StaffStore + AccountDirectory> Stored<S> {
    fn store(&self) -> Result<&Arc<S>, String> {
        self.0.get().ok_or_else(|| {
            "this tab has no accounts store yet: no request with it on its context has come\n\
             help: register the auth plugin, which puts its store on the request context"
                .to_string()
        })
    }
}

fn store_failed(e: crate::storage::StoreError) -> String {
    format!("the account store failed: {e}")
}

#[async_trait]
impl<S: StaffStore + AccountDirectory> RolesData for Stored<S> {
    async fn read(&self, _who: &Principal<'_>) -> Result<RolesView, String> {
        let store = self.store()?;
        let mut roles = Vec::new();
        for role in store.roles().await.map_err(store_failed)? {
            let mut members = Vec::new();
            for id in store.members(&role.name).await.map_err(store_failed)? {
                let label = match store.account(&id).await.map_err(store_failed)? {
                    Some(account) => account
                        .email
                        .map_or_else(|| id.to_string(), |e| e.to_string()),
                    None => id.to_string(),
                };
                members.push((id.to_string(), label));
            }
            roles.push(RoleView {
                name: role.name,
                permissions: role.permissions.into_iter().collect(),
                members,
            });
        }
        Ok(RolesView {
            durable: store.is_durable(),
            roles,
        })
    }

    async fn change(
        &self,
        _who: &Principal<'_>,
        change: RolesChange,
    ) -> Result<RolesOutcome, String> {
        let store = self.store()?;
        let refused = |refusal: Refusal| Ok(RolesOutcome::from(Some(refusal)));
        match change {
            RolesChange::Define { role, permissions } => {
                if !valid_name(&role) {
                    return refused(Refusal::BadName);
                }
                let role = Role::new(role, permissions.into_iter().filter(|p| !p.is_empty()));
                store.put_role(&role).await.map_err(store_failed)?;
            }
            RolesChange::Delete { role } => {
                store.delete_role(&role).await.map_err(store_failed)?;
            }
            RolesChange::Assign { role, email } => {
                let Ok(address) = EmailAddress::parse(&email) else {
                    return refused(Refusal::NotAnAddress);
                };
                let Some(account) = store
                    .account_by_email(&address)
                    .await
                    .map_err(store_failed)?
                else {
                    return refused(Refusal::NoAccount);
                };
                if !store
                    .assign_role(&account.id, &role)
                    .await
                    .map_err(store_failed)?
                {
                    return refused(Refusal::NoRole);
                }
            }
            RolesChange::Unassign { role, account } => {
                let account = AccountId::new(account);
                store
                    .unassign_role(&account, &role)
                    .await
                    .map_err(store_failed)?;
            }
            RolesChange::Grant {
                role,
                resource,
                level,
                give,
            } => {
                let Some(level) = Level::parse(&level) else {
                    return refused(Refusal::NoLevel);
                };
                if !valid_resource(&resource) {
                    return refused(Refusal::BadResource);
                }
                let roles = store.roles().await.map_err(store_failed)?;
                let Some(mut found) = roles.into_iter().find(|candidate| candidate.name == role)
                else {
                    return refused(Refusal::NoRole);
                };
                let permission = level.on(&resource);
                if give {
                    found.permissions.insert(permission);
                } else {
                    found.permissions.remove(&permission);
                }
                store.put_role(&found).await.map_err(store_failed)?;
            }
        }
        Ok(RolesOutcome::Done)
    }
}

/// The data side of the roles tab on the backoffice wire: decodes a
/// [`RolesCall`], checks it with [`RolesCall::allows`], and runs it on a
/// [`RolesData`].
pub struct RolesServe(Arc<dyn RolesData>);

impl RolesServe {
    /// Serves `data`.
    pub fn new(data: Arc<dyn RolesData>) -> RolesServe {
        RolesServe(data)
    }

    /// As the wire server takes it.
    pub fn shared(self) -> Arc<dyn PanelServe> {
        Arc::new(self)
    }
}

impl std::fmt::Debug for RolesServe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RolesServe")
    }
}

fn decode(call: &str) -> Result<RolesCall, String> {
    serde_json::from_str(call).map_err(|e| format!("a roles call did not decode: {e}"))
}

#[async_trait]
impl PanelServe for RolesServe {
    fn level(&self, call: &str) -> Result<Level, String> {
        decode(call).map(|call| call.level())
    }

    fn allows(&self, _tab: &str, call: &str, grants: &Grants) -> Result<bool, String> {
        decode(call).map(|call| call.allows(grants))
    }

    async fn serve(&self, who: &Principal<'_>, call: &str) -> Result<String, String> {
        let answer = match decode(call)? {
            RolesCall::Read => RolesAnswer::View(self.0.read(who).await?),
            RolesCall::Change(change) => RolesAnswer::Outcome(self.0.change(who, change).await?),
        };
        serde_json::to_string(&answer).map_err(|e| format!("a roles answer did not encode: {e}"))
    }
}

/// The roles tab's data in another process, through the backoffice wire.
pub struct RemoteRoles(Arc<dyn PanelWire>);

impl RemoteRoles {
    /// The roles behind `wire`.
    pub fn new(wire: Arc<dyn PanelWire>) -> RemoteRoles {
        RemoteRoles(wire)
    }

    async fn ask(&self, who: &Principal<'_>, call: &RolesCall) -> Result<RolesAnswer, String> {
        let text =
            serde_json::to_string(call).map_err(|e| format!("a roles call did not encode: {e}"))?;
        let answer = self.0.send(who, ROLES_KEY, text).await?;
        serde_json::from_str(&answer).map_err(|e| format!("a roles answer did not decode: {e}"))
    }
}

impl std::fmt::Debug for RemoteRoles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RemoteRoles")
    }
}

#[async_trait]
impl RolesData for RemoteRoles {
    async fn read(&self, who: &Principal<'_>) -> Result<RolesView, String> {
        match self.ask(who, &RolesCall::Read).await? {
            RolesAnswer::View(view) => Ok(view),
            RolesAnswer::Outcome(_) => Err("the data side answered a read with an outcome".into()),
        }
    }

    async fn change(
        &self,
        who: &Principal<'_>,
        change: RolesChange,
    ) -> Result<RolesOutcome, String> {
        match self.ask(who, &RolesCall::Change(change)).await? {
            RolesAnswer::Outcome(outcome) => Ok(outcome),
            RolesAnswer::View(_) => Err("the data side answered a change with the roles".into()),
        }
    }
}

/// Where the tab reads its roles.
enum Source<S> {
    /// The store, which arrives with the first request.
    Local(Late<Arc<S>>),
    /// Any data side: another process, through the wire.
    Data(Arc<dyn RolesData>),
}

/// Roles as a tab: who holds which permissions, and the buttons that change
/// it.
pub struct RolesPanel<S: StaffStore + AccountDirectory> {
    source: Source<S>,
}

impl<S: StaffStore + AccountDirectory> RolesPanel<S> {
    /// The tab over a store the application already holds.
    pub fn new(store: Arc<S>) -> Self {
        Self::late(Late::ready(store))
    }

    /// The tab over a store that arrives later, in `cell`: the admin fills
    /// it from the first request that carries the auth services.
    pub fn late(cell: Late<Arc<S>>) -> Self {
        Self {
            source: Source::Local(cell),
        }
    }

    /// The tab over any [`RolesData`]: [`RemoteRoles`] in the backoffice
    /// binary, which reads the roles through the wire (§7.6).
    pub fn over(data: Arc<dyn RolesData>) -> Self {
        Self {
            source: Source::Data(data),
        }
    }

    fn data(&self) -> Arc<dyn RolesData> {
        match &self.source {
            Source::Local(cell) => Arc::new(Stored(cell.clone())),
            Source::Data(data) => Arc::clone(data),
        }
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

    fn serve(&self) -> Option<Arc<dyn PanelServe>> {
        match &self.source {
            Source::Local(_) => Some(RolesServe::new(self.data()).shared()),
            Source::Data(_) => None,
        }
    }

    fn remote(&self, wire: Arc<dyn PanelWire>) -> Option<Arc<dyn Panel>> {
        Some(Arc::new(RolesPanel::<S>::over(Arc::new(RemoteRoles::new(
            wire,
        )))))
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
        let view = self.data().read(&r.principal).await.map_err(failed)?;
        let grantor = Grantor::of(&r.principal);
        let rows = view
            .roles
            .into_iter()
            .map(|role| RoleRow {
                permissions: role.permissions.join(" "),
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
                members: role
                    .members
                    .into_iter()
                    .map(|(account, label)| MemberRow { account, label })
                    .collect(),
            })
            .collect();
        let ctx = RolesCtx {
            volatile: !view.durable,
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
        let change = match action.as_str() {
            "define" => {
                if !valid_name(&name) {
                    return Ok(back(Some(Refusal::BadName)));
                }
                RolesChange::Define {
                    role: name,
                    permissions: field("permissions")
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter(|p| !p.is_empty())
                        .map(str::to_string)
                        .collect(),
                }
            }
            "delete" => RolesChange::Delete { role: name },
            "assign" => RolesChange::Assign {
                role: name,
                email: field("email"),
            },
            "unassign" => RolesChange::Unassign {
                role: name,
                account: field("account"),
            },
            _ => return Ok(back(Some(Refusal::NoAction))),
        };
        self.changed(r, change).await
    }

    /// Runs `change` and goes back to the tab, with the refusal's code.
    async fn changed(
        &self,
        r: &PanelRequest<'_>,
        change: RolesChange,
    ) -> Result<PanelPage, AdminError> {
        let outcome = self
            .data()
            .change(&r.principal, change)
            .await
            .map_err(failed)?;
        Ok(match outcome {
            RolesOutcome::Done => PanelPage::redirect(r.url("")),
            // A code the tab did not write shows nothing (`Refusal`).
            RolesOutcome::Refused(code) => {
                let code = Refusal::from_code(&code).map_or("", Refusal::code);
                PanelPage::redirect(format!("{}?error={code}", r.url("")))
            }
        })
    }

    /// Gives or takes one level on one resource to or from a role.
    async fn grant(
        &self,
        r: &PanelRequest<'_>,
        grantor: &Grantor,
        role: &str,
        give: bool,
    ) -> Result<PanelPage, AdminError> {
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
        let change = RolesChange::Grant {
            role: role.to_string(),
            resource,
            level: level.as_str().to_string(),
            give,
        };
        self.changed(r, change).await
    }
}

fn failed(e: String) -> AdminError {
    AdminError::Render(rsx::Error::custom(e))
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
