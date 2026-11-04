//! [`Orm`]: SeaORM over the pool [`brocanth_db::Db`] opened.
//!
//! Behind the `db` feature. ADR 2 sets the order: the pool comes first, and
//! the ORM is registered after it because it needs it. So this type does not
//! open a pool. It wraps the one `brocanth-db` holds, and a sea-query
//! statement through `Db` and an entity query through `Orm` use the same
//! connections, the same session settings and the same limits.
//!
//! ```no_run
//! use brocanth_db::{Db, DbConfig};
//! use brocanth_model::sea_orm::TransactionTrait;
//! use brocanth_model::Orm;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let db = Db::connect(&DbConfig::new("postgres://localhost/app")).await?;
//! let orm = Orm::from_db(&db);
//! // `Orm` derefs to `DatabaseConnection`, so every SeaORM call takes
//! // `&*orm` or `orm.connection()`, and a transaction is a method on it.
//! let tx = orm.begin().await?;
//! tx.commit().await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Why a newtype and not an alias
//!
//! A `type Orm = DatabaseConnection` cannot be a key. The plugin puts the
//! handle into the application's services and onto the request context, and
//! both are keyed by type. `ContextItem` is the core's trait and
//! `DatabaseConnection` is SeaORM's type, so no crate may implement one for
//! the other. A local type can. It also says which connection it is: the
//! one over the application's shared pool, and not some other connection a
//! plugin opened.
//!
//! The newtype costs nothing at the call site. It derefs to
//! `DatabaseConnection`, and [`Orm::connection`] and
//! `From<Orm> for DatabaseConnection` hand the connection to an API that
//! names the SeaORM type, such as `brocanth_admin_model::admin_source!`.

use std::ops::Deref;

use brocanth_db::Db;
use sea_orm::{DatabaseConnection, SqlxPostgresConnector};

/// SeaORM's connection over the pool a [`Db`] holds.
///
/// A clone is cheap: it shares the pool.
#[derive(Clone, Debug)]
pub struct Orm(DatabaseConnection);

impl Orm {
    /// The ORM over `db`'s pool. Opens no connection.
    pub fn from_db(db: &Db) -> Orm {
        Orm(SqlxPostgresConnector::from_sqlx_postgres_pool(
            db.pool().clone(),
        ))
    }

    /// The SeaORM connection, for a call that takes `&C: ConnectionTrait`.
    pub fn connection(&self) -> &DatabaseConnection {
        &self.0
    }

    /// The SeaORM connection, by value.
    pub fn into_connection(self) -> DatabaseConnection {
        self.0
    }
}

impl Deref for Orm {
    type Target = DatabaseConnection;

    fn deref(&self) -> &DatabaseConnection {
        &self.0
    }
}

impl From<&Db> for Orm {
    fn from(db: &Db) -> Orm {
        Orm::from_db(db)
    }
}

impl From<Orm> for DatabaseConnection {
    fn from(orm: Orm) -> DatabaseConnection {
        orm.0
    }
}

/// The plugin publishes the handle on the request context, so a handler
/// reads it with `ctx.provided::<Orm, _>()`.
#[cfg(feature = "plugin")]
impl brocanth::plugin_context::ContextItem for Orm {}
