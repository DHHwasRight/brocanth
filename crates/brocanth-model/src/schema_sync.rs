//! The one adapter between brocanth and SeaORM's `schema-sync`.
//!
//! `schema-sync` is self-described as experimental and exempt from semver, and
//! it only adds: it never drops a table, never drops a column, and when a
//! column's type differs from the entity it logs a warning and moves on. It is
//! Django's `migrate` for the additive eighty per cent and nothing at all for
//! the destructive twenty.
//!
//! So brocanth does not call it. Migrations stay hand-written, as the plan
//! says. What lives here is the *seam*: one trait, so the day `schema-sync`
//! stabilises — or breaks — the cost is one implementation, not a rewrite of
//! every call site.
//!
//! Nothing in brocanth-model depends on SeaORM's `schema-sync` symbols today.
//! [`PlanOnly`] is the shipped implementation: it reports what the entity
//! declares and applies nothing.

use sea_orm::{DbBackend, EntityTrait, Schema};

/// One schema change a planner proposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// SQL to run.
    pub sql: String,
    /// `true` when the statement can drop or rewrite data.
    pub destructive: bool,
}

/// What a planner found.
///
/// `warnings` is where a planner that cannot express a change says so, rather
/// than applying something else. A rename, a backfill and a type change all
/// land here: the type system cannot tell a rename from a drop-plus-add, so the
/// generator proposes and the developer commits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    /// Statements that only add.
    pub additive: Vec<Statement>,
    /// Statements that can lose data.
    pub destructive: Vec<Statement>,
    /// Changes the planner saw but will not express.
    pub warnings: Vec<String>,
}

impl Plan {
    /// `true` when there is nothing to do.
    pub fn is_empty(&self) -> bool {
        self.additive.is_empty() && self.destructive.is_empty()
    }

    /// `true` when applying this plan can lose data.
    pub fn is_destructive(&self) -> bool {
        !self.destructive.is_empty()
    }
}

/// Produces a [`Plan`] from the models an application declares.
///
/// The seam. Swap the implementation, not the callers.
pub trait SchemaPlanner {
    /// Propose the statements that would bring a database in line with the
    /// models. A planner never applies anything; brocanth writes migrations the
    /// developer reads, edits and commits.
    fn plan(&self) -> Plan;
}

/// The shipped planner: renders what each registered entity declares, marks
/// nothing destructive, and applies nothing.
#[derive(Debug, Clone)]
pub struct PlanOnly {
    backend: DbBackend,
    statements: Vec<String>,
}

impl PlanOnly {
    /// A planner for one backend.
    pub fn new(backend: DbBackend) -> Self {
        Self {
            backend,
            statements: Vec::new(),
        }
    }

    /// Add the `CREATE TABLE` an entity declares.
    pub fn with<E: EntityTrait>(mut self, entity: E) -> Self {
        let schema = Schema::new(self.backend);
        let create = schema.create_table_from_entity(entity);
        self.statements
            .push(self.backend.build(&create).to_string());
        self
    }
}

impl SchemaPlanner for PlanOnly {
    fn plan(&self) -> Plan {
        Plan {
            additive: self
                .statements
                .iter()
                .map(|sql| Statement {
                    sql: sql.clone(),
                    destructive: false,
                })
                .collect(),
            destructive: Vec::new(),
            warnings: vec![
                "schema-sync is experimental and exempt from semver; brocanth keeps migrations \
                 hand-written. This plan is a proposal, not an applied change."
                    .to_owned(),
            ],
        }
    }
}
