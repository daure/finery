use std::{collections::HashMap, env, fs, path::PathBuf};

use sqlx::{
    Any, AnyPool, AssertSqlSafe, ConnectOptions, Row, Transaction,
    any::{AnyConnectOptions, AnyPoolOptions},
    migrate::Migrator,
    sqlite::SqliteConnectOptions,
};

use crate::store::composer::{ChangeSet, SubmissionAttempt, TicketChange};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SqlDialect {
    Sqlite,
    Postgres,
}

impl SqlDialect {
    fn from_url(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if url.starts_with("sqlite:") {
            Ok(Self::Sqlite)
        } else if url.starts_with("postgres:") || url.starts_with("postgresql:") {
            Ok(Self::Postgres)
        } else {
            Err(format!("unsupported FINERY_DATABASE_URL: {url}").into())
        }
    }

    fn placeholder(self, index: usize) -> String {
        match self {
            Self::Sqlite => "?".into(),
            Self::Postgres => format!("${index}"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Storage {
    pool: AnyPool,
    dialect: SqlDialect,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionedChangeSet {
    pub change_set: ChangeSet,
    pub revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VersionedChangeSetCatalog {
    pub change_sets: Vec<VersionedChangeSet>,
    pub catalog_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConditionalSaveChangeSetOutcome {
    Saved {
        change_set_revision: i64,
        catalog_revision: i64,
    },
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConditionalDeleteChangeSetOutcome {
    Deleted { catalog_revision: i64 },
    Conflict,
}

impl Storage {
    pub(crate) async fn connect_from_env() -> Result<Self, Box<dyn std::error::Error>> {
        match env::var("FINERY_DATABASE_URL") {
            Ok(url) if url.trim().is_empty() => Err("FINERY_DATABASE_URL must not be empty".into()),
            Ok(url) => Self::connect(&url).await,
            Err(env::VarError::NotPresent) => {
                let path = default_sqlite_path()?;
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                Self::connect_sqlite_path(path).await
            }
            Err(error) => Err(error.into()),
        }
    }

    async fn connect(url: &str) -> Result<Self, Box<dyn std::error::Error>> {
        sqlx::any::install_default_drivers();
        let dialect = SqlDialect::from_url(url)?;
        let options = if dialect == SqlDialect::Sqlite && url.contains(":memory:") {
            pool_options(dialect).max_connections(1)
        } else {
            pool_options(dialect)
        };
        let pool = options.connect(url).await?;
        let storage = Self { pool, dialect };
        storage.configure().await?;
        Ok(storage)
    }

    #[cfg(test)]
    pub(crate) async fn connect_for_tests() -> Result<Self, Box<dyn std::error::Error>> {
        Self::connect("sqlite::memory:").await
    }

    async fn connect_sqlite_path(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        sqlx::any::install_default_drivers();
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let any_options = AnyConnectOptions::from_url(&options.to_url_lossy())?;
        let pool = pool_options(SqlDialect::Sqlite)
            .connect_with(any_options)
            .await?;
        let storage = Self {
            pool,
            dialect: SqlDialect::Sqlite,
        };
        storage.configure().await?;
        Ok(storage)
    }

    async fn configure(&self) -> Result<(), Box<dyn std::error::Error>> {
        if self.dialect == SqlDialect::Sqlite {
            sqlx::query("PRAGMA journal_mode = WAL")
                .execute(&self.pool)
                .await?;
        }
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) async fn load_change_sets(
        &self,
    ) -> Result<Vec<ChangeSet>, Box<dyn std::error::Error>> {
        Ok(self
            .load_versioned_change_sets()
            .await?
            .change_sets
            .into_iter()
            .map(|set| set.change_set)
            .collect())
    }

    pub(crate) async fn load_change_set(
        &self,
        id: &str,
    ) -> Result<Option<VersionedChangeSet>, Box<dyn std::error::Error>> {
        let mut transaction = self.pool.begin().await?;
        if self.dialect == SqlDialect::Postgres {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *transaction)
                .await?;
        }
        let closed_column = match self.dialect {
            SqlDialect::Sqlite => "CAST(closed AS INTEGER) AS closed",
            SqlDialect::Postgres => "closed",
        };
        let query = format!(
            "SELECT public_id, name, selected_ticket_ids, submission_attempt, {closed_column}, revision FROM change_sets WHERE public_id = {}",
            self.dialect.placeholder(1)
        );
        let Some(row) = sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .fetch_optional(&mut *transaction)
            .await?
        else {
            transaction.commit().await?;
            return Ok(None);
        };

        let change_set = self
            .versioned_change_set_from_row_in_transaction(row, &mut transaction)
            .await?;
        transaction.commit().await?;
        Ok(Some(change_set))
    }

    pub(crate) async fn load_versioned_change_sets(
        &self,
    ) -> Result<VersionedChangeSetCatalog, Box<dyn std::error::Error>> {
        let mut transaction = self.pool.begin().await?;
        if self.dialect == SqlDialect::Postgres {
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .execute(&mut *transaction)
                .await?;
        }
        let closed_column = match self.dialect {
            SqlDialect::Sqlite => "CAST(closed AS INTEGER) AS closed",
            SqlDialect::Postgres => "closed",
        };
        let query = format!(
            "SELECT public_id, name, selected_ticket_ids, submission_attempt, {closed_column}, revision FROM change_sets ORDER BY created_at, public_id"
        );
        let rows = sqlx::query(AssertSqlSafe(query.as_str()))
            .fetch_all(&mut *transaction)
            .await?;
        let mut sets = Vec::with_capacity(rows.len());
        for row in rows {
            sets.push(
                self.versioned_change_set_from_row_in_transaction(row, &mut transaction)
                    .await?,
            );
        }
        let catalog_revision = sqlx::query("SELECT revision FROM change_set_catalog WHERE id = 1")
            .fetch_one(&mut *transaction)
            .await?
            .try_get("revision")?;
        transaction.commit().await?;
        Ok(VersionedChangeSetCatalog {
            change_sets: sets,
            catalog_revision,
        })
    }

    pub(crate) async fn load_change_set_catalog_revision(
        &self,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        Ok(
            sqlx::query("SELECT revision FROM change_set_catalog WHERE id = 1")
                .fetch_one(&self.pool)
                .await?
                .try_get("revision")?,
        )
    }

    #[allow(dead_code)]
    pub(crate) async fn save_change_set(
        &self,
        set: &ChangeSet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut transaction = self.pool.begin().await?;
        let upsert = format!(
            "INSERT INTO change_sets (public_id, name, selected_ticket_ids, submission_attempt, closed) VALUES ({}, {}, {}, {}, {}) ON CONFLICT (public_id) DO UPDATE SET name = excluded.name, selected_ticket_ids = excluded.selected_ticket_ids, submission_attempt = excluded.submission_attempt, closed = excluded.closed, revision = change_sets.revision + 1, updated_at = CURRENT_TIMESTAMP",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4),
            self.dialect.placeholder(5)
        );
        sqlx::query(AssertSqlSafe(upsert.as_str()))
            .bind(&set.id)
            .bind(&set.name)
            .bind(serde_json::to_string(&set.selected_ticket_ids)?)
            .bind(serde_json::to_string(&set.submission_attempt)?)
            .bind(set.closed)
            .execute(&mut *transaction)
            .await?;
        self.replace_ticket_changes(&mut transaction, set).await?;
        self.increment_catalog_revision(&mut transaction).await?;
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn save_change_set_if_revision(
        &self,
        set: &ChangeSet,
        expected_revision: Option<i64>,
    ) -> Result<ConditionalSaveChangeSetOutcome, Box<dyn std::error::Error>> {
        let mut transaction = self.pool.begin().await?;
        let change_set_revision = match expected_revision {
            Some(revision) => {
                let update = format!(
                    "UPDATE change_sets SET name = {}, selected_ticket_ids = {}, submission_attempt = {}, closed = {}, revision = revision + 1, updated_at = CURRENT_TIMESTAMP WHERE public_id = {} AND revision = {}",
                    self.dialect.placeholder(1),
                    self.dialect.placeholder(2),
                    self.dialect.placeholder(3),
                    self.dialect.placeholder(4),
                    self.dialect.placeholder(5),
                    self.dialect.placeholder(6)
                );
                let result = sqlx::query(AssertSqlSafe(update.as_str()))
                    .bind(&set.name)
                    .bind(serde_json::to_string(&set.selected_ticket_ids)?)
                    .bind(serde_json::to_string(&set.submission_attempt)?)
                    .bind(set.closed)
                    .bind(&set.id)
                    .bind(revision)
                    .execute(&mut *transaction)
                    .await?;
                if result.rows_affected() == 0 {
                    return Ok(ConditionalSaveChangeSetOutcome::Conflict);
                }
                revision + 1
            }
            None => {
                let insert = format!(
                    "INSERT INTO change_sets (public_id, name, selected_ticket_ids, submission_attempt, closed) VALUES ({}, {}, {}, {}, {}) ON CONFLICT (public_id) DO NOTHING",
                    self.dialect.placeholder(1),
                    self.dialect.placeholder(2),
                    self.dialect.placeholder(3),
                    self.dialect.placeholder(4),
                    self.dialect.placeholder(5)
                );
                let result = sqlx::query(AssertSqlSafe(insert.as_str()))
                    .bind(&set.id)
                    .bind(&set.name)
                    .bind(serde_json::to_string(&set.selected_ticket_ids)?)
                    .bind(serde_json::to_string(&set.submission_attempt)?)
                    .bind(set.closed)
                    .execute(&mut *transaction)
                    .await?;
                if result.rows_affected() == 0 {
                    return Ok(ConditionalSaveChangeSetOutcome::Conflict);
                }
                1
            }
        };
        self.replace_ticket_changes(&mut transaction, set).await?;
        let catalog_revision = self.increment_catalog_revision(&mut transaction).await?;
        transaction.commit().await?;
        Ok(ConditionalSaveChangeSetOutcome::Saved {
            change_set_revision,
            catalog_revision,
        })
    }

    async fn versioned_change_set_from_row_in_transaction(
        &self,
        row: sqlx::any::AnyRow,
        transaction: &mut Transaction<'_, Any>,
    ) -> Result<VersionedChangeSet, Box<dyn std::error::Error>> {
        let id: String = row.try_get("public_id")?;
        let closed = match self.dialect {
            SqlDialect::Sqlite => row.try_get::<i64, _>("closed")? != 0,
            SqlDialect::Postgres => row.try_get("closed")?,
        };
        let query = format!(
            "SELECT sibling_order, payload FROM ticket_changes WHERE change_set_id = {} ORDER BY sibling_order, ticket_id",
            self.dialect.placeholder(1)
        );
        let tickets = sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(&id)
            .fetch_all(&mut **transaction)
            .await?
            .into_iter()
            .map(|row| {
                let payload: String = row.try_get("payload")?;
                let mut change = serde_json::from_str::<TicketChange>(&payload)?;
                change.sibling_order = usize::try_from(row.try_get::<i64, _>("sibling_order")?)?;
                Ok(change)
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        Ok(VersionedChangeSet {
            change_set: ChangeSet {
                id,
                name: row.try_get("name")?,
                tickets,
                selected_ticket_ids: serde_json::from_str(
                    &row.try_get::<String, _>("selected_ticket_ids")?,
                )?,
                submission_attempt: row
                    .try_get::<Option<String>, _>("submission_attempt")?
                    .map(|value| serde_json::from_str::<Option<SubmissionAttempt>>(&value))
                    .transpose()?
                    .flatten(),
                closed,
            },
            revision: row.try_get("revision")?,
        })
    }

    async fn replace_ticket_changes(
        &self,
        transaction: &mut Transaction<'_, Any>,
        set: &ChangeSet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let delete = format!(
            "DELETE FROM ticket_changes WHERE change_set_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(delete.as_str()))
            .bind(&set.id)
            .execute(&mut **transaction)
            .await?;
        let insert = format!(
            "INSERT INTO ticket_changes (change_set_id, ticket_id, sibling_order, payload) VALUES ({}, {}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3),
            self.dialect.placeholder(4)
        );
        for change in &set.tickets {
            sqlx::query(AssertSqlSafe(insert.as_str()))
                .bind(&set.id)
                .bind(&change.id)
                .bind(i64::try_from(change.sibling_order)?)
                .bind(serde_json::to_string(change)?)
                .execute(&mut **transaction)
                .await?;
        }
        Ok(())
    }

    async fn increment_catalog_revision(
        &self,
        transaction: &mut Transaction<'_, Any>,
    ) -> Result<i64, Box<dyn std::error::Error>> {
        sqlx::query("UPDATE change_set_catalog SET revision = revision + 1 WHERE id = 1")
            .execute(&mut **transaction)
            .await?;
        Ok(
            sqlx::query("SELECT revision FROM change_set_catalog WHERE id = 1")
                .fetch_one(&mut **transaction)
                .await?
                .try_get("revision")?,
        )
    }

    #[allow(dead_code)]
    pub(crate) async fn delete_change_set(
        &self,
        id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = format!(
            "DELETE FROM change_sets WHERE public_id = {}",
            self.dialect.placeholder(1)
        );
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .execute(&mut *transaction)
            .await?;
        if result.rows_affected() != 0 {
            self.increment_catalog_revision(&mut transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_change_set_if_revision(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<ConditionalDeleteChangeSetOutcome, Box<dyn std::error::Error>> {
        let query = format!(
            "DELETE FROM change_sets WHERE public_id = {} AND revision = {}",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        let mut transaction = self.pool.begin().await?;
        let result = sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .bind(expected_revision)
            .execute(&mut *transaction)
            .await?;
        if result.rows_affected() == 0 {
            return Ok(ConditionalDeleteChangeSetOutcome::Conflict);
        }
        let catalog_revision = self.increment_catalog_revision(&mut transaction).await?;
        transaction.commit().await?;
        Ok(ConditionalDeleteChangeSetOutcome::Deleted { catalog_revision })
    }

    pub(crate) async fn load_settings(
        &self,
    ) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
        sqlx::query("SELECT key, value FROM app_settings")
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| Ok((row.try_get("key")?, row.try_get("value")?)))
            .collect()
    }

    pub(crate) async fn set_settings(
        &self,
        values: &[(&str, String)],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = format!(
            "INSERT INTO app_settings (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        let mut transaction = self.pool.begin().await?;
        for (key, value) in values {
            sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(key)
                .bind(value)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

fn pool_options(dialect: SqlDialect) -> AnyPoolOptions {
    let options = AnyPoolOptions::new().max_connections(5);
    if dialect == SqlDialect::Sqlite {
        options.after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("PRAGMA foreign_keys = ON")
                    .execute(&mut *connection)
                    .await?;
                sqlx::query("PRAGMA busy_timeout = 5000")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        })
    } else {
        options
    }
}

fn default_sqlite_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let base = if cfg!(target_os = "linux") {
        match env::var_os("XDG_DATA_HOME") {
            Some(value) if value.is_empty() => return Err("XDG_DATA_HOME must not be empty".into()),
            Some(value) => {
                let path = PathBuf::from(value);
                if !path.is_absolute() {
                    return Err(
                        format!("XDG_DATA_HOME must be absolute: {}", path.display()).into(),
                    );
                }
                path
            }
            None => dirs::data_local_dir().ok_or("could not determine platform data directory")?,
        }
    } else {
        dirs::data_local_dir().ok_or("could not determine platform data directory")?
    };
    Ok(base.join("finery").join("finery.sqlite"))
}

#[cfg(test)]
mod tests;
