//! A `#[db]` policy as phase 2 of docs/policy-rls-bridge.md will emit it:
//! `Rules` written by hand, and `impl Policy` that resolves them.
#![allow(missing_docs)]

use brocanth_policy::{Guard, Missing, Op, Operand, Rule, Rules, Term, Value};

#[derive(Clone, Debug, Default, brocanth_policy::Subject)]
pub struct Visitor {
    #[attr]
    pub user_id: Option<i64>,
    #[attr]
    pub tenant_id: i64,
    #[attr]
    pub admin: bool,
    #[attr]
    pub banned: bool,
    #[attr]
    pub role: String,
}

brocanth_policy::policy! {
    pub entity Post in "posts" {
        #[key] id: i64,
        author_id: Option<i64>,
        tenant_id: i64,
        status: String,
        title: String,
    }

    /// [`post_rules`], written with `policy!` and `#[db]`.
    #[db]
    pub policy PostDb(who: &Visitor) for Post {
        list {
            who.banned => never,
            who.admin => always,
            tenant_id == who.tenant_id && (status == "published" || author_id == who.user_id else deny),
        }
        view: author_id == who.user_id else deny,
        create: tenant_id == who.tenant_id && author_id == who.user_id else deny,
        update {
            who.admin => always,
            author_id == who.user_id else deny && status == "draft",
        }
        update_check {
            who.admin => always,
            author_id == who.user_id else deny,
        }
    }
}

pub fn attr(key: &'static str, missing: Missing) -> Operand {
    Operand::Attr { key, missing }
}

pub fn lit(v: impl Into<Value>) -> Operand {
    Operand::Value(v.into())
}

pub fn cmp(column: PostColumn, op: Op, operand: Operand) -> Term<PostColumn> {
    Term::Cmp {
        column,
        op,
        operand,
    }
}

/// A tenant's published posts and the author's own; admins see all,
/// banned visitors see none in a list. Authors update their drafts, and may
/// publish them (`update_check` drops the status). No one deletes.
pub fn post_rules() -> Rules<Post> {
    use PostColumn::*;
    let owner = cmp(AuthorId, Op::Eq, attr("user_id", Missing::Deny));
    let tenant = cmp(TenantId, Op::Eq, attr("tenant_id", Missing::Deny));
    let mut rules = Rules::deny_all();
    rules.list = Rule::new(Term::All(vec![
        tenant.clone(),
        Term::Any(vec![cmp(Status, Op::Eq, lit("published")), owner.clone()]),
    ]))
    .arm(Guard::Attr("banned"), Term::Never)
    .arm(Guard::Attr("admin"), Term::Always);
    rules.view = Some(Rule::new(owner.clone()));
    rules.create = Rule::new(Term::All(vec![tenant, owner.clone()]));
    rules.update = Rule::new(Term::All(vec![
        owner.clone(),
        cmp(Status, Op::Eq, lit("draft")),
    ]))
    .arm(Guard::Attr("admin"), Term::Always);
    rules.update_check = Some(Rule::new(owner).arm(Guard::Attr("admin"), Term::Always));
    // `delete` stays `never`: no policy, no grant.
    rules
}

/// `impl Policy` and `impl DbPolicy` for `$name`, from `$rules`.
macro_rules! db_policy {
    ($name:ident, $rules:expr) => {
        impl brocanth_policy::Policy for $name {
            type Entity = $crate::common::policy::Post;
            type Subject = $crate::common::policy::Visitor;
            fn list(&self, who: &Self::Subject) -> brocanth_policy::EntityPred<Self::Entity> {
                <Self as brocanth_policy::DbPolicy>::rules()
                    .list
                    .resolve(&brocanth_policy::Subject::attributes(who))
            }
            fn view(&self, who: &Self::Subject) -> brocanth_policy::EntityPred<Self::Entity> {
                <Self as brocanth_policy::DbPolicy>::rules()
                    .view()
                    .resolve(&brocanth_policy::Subject::attributes(who))
            }
            fn create(&self, who: &Self::Subject) -> brocanth_policy::EntityPred<Self::Entity> {
                <Self as brocanth_policy::DbPolicy>::rules()
                    .create
                    .resolve(&brocanth_policy::Subject::attributes(who))
            }
            fn update(&self, who: &Self::Subject) -> brocanth_policy::EntityPred<Self::Entity> {
                <Self as brocanth_policy::DbPolicy>::rules()
                    .update
                    .resolve(&brocanth_policy::Subject::attributes(who))
            }
            fn delete(&self, who: &Self::Subject) -> brocanth_policy::EntityPred<Self::Entity> {
                <Self as brocanth_policy::DbPolicy>::rules()
                    .delete
                    .resolve(&brocanth_policy::Subject::attributes(who))
            }
        }
        impl brocanth_policy::DbPolicy for $name {
            fn rules() -> brocanth_policy::Rules<$crate::common::policy::Post> {
                $rules
            }
        }
    };
}
#[allow(unused_imports)]
pub(crate) use db_policy;

pub struct PostPolicy;
db_policy!(PostPolicy, post_rules());
