use std::{collections::HashMap, env, fs, path::PathBuf};

use sqlx::{
    AnyPool, AssertSqlSafe, ConnectOptions, Row,
    any::{AnyConnectOptions, AnyPoolOptions},
    migrate::Migrator,
    sqlite::SqliteConnectOptions,
};

use crate::store::composer::{ChangeSet, TicketChange};

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

    pub(crate) async fn load_change_sets(
        &self,
    ) -> Result<Vec<ChangeSet>, Box<dyn std::error::Error>> {
        let rows =
            sqlx::query("SELECT public_id, name FROM change_sets ORDER BY created_at, public_id")
                .fetch_all(&self.pool)
                .await?;
        let mut sets = Vec::with_capacity(rows.len());
        for row in rows {
            let id: String = row.try_get("public_id")?;
            let query = format!(
                "SELECT payload FROM ticket_changes WHERE change_set_id = {} ORDER BY ticket_id",
                self.dialect.placeholder(1)
            );
            let changes = sqlx::query(AssertSqlSafe(query.as_str()))
                .bind(&id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|row| {
                    let payload: String = row.try_get("payload")?;
                    Ok(serde_json::from_str::<TicketChange>(&payload)?)
                })
                .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
            sets.push(ChangeSet {
                id,
                name: row.try_get("name")?,
                tickets: changes,
            });
        }
        Ok(sets)
    }

    pub(crate) async fn save_change_set(
        &self,
        set: &ChangeSet,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut transaction = self.pool.begin().await?;
        let upsert = format!(
            "INSERT INTO change_sets (public_id, name) VALUES ({}, {}) ON CONFLICT (public_id) DO UPDATE SET name = excluded.name, updated_at = CURRENT_TIMESTAMP",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        sqlx::query(AssertSqlSafe(upsert.as_str()))
            .bind(&set.id)
            .bind(&set.name)
            .execute(&mut *transaction)
            .await?;
        let delete = format!(
            "DELETE FROM ticket_changes WHERE change_set_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(delete.as_str()))
            .bind(&set.id)
            .execute(&mut *transaction)
            .await?;
        let insert = format!(
            "INSERT INTO ticket_changes (change_set_id, ticket_id, payload) VALUES ({}, {}, {})",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2),
            self.dialect.placeholder(3)
        );
        for change in &set.tickets {
            sqlx::query(AssertSqlSafe(insert.as_str()))
                .bind(&set.id)
                .bind(&change.id)
                .bind(serde_json::to_string(change)?)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_change_set(
        &self,
        id: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = format!(
            "DELETE FROM change_sets WHERE public_id = {}",
            self.dialect.placeholder(1)
        );
        sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
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

    pub(crate) async fn set_setting(
        &self,
        key: &str,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let query = format!(
            "INSERT INTO app_settings (key, value) VALUES ({}, {}) ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            self.dialect.placeholder(1),
            self.dialect.placeholder(2)
        );
        sqlx::query(AssertSqlSafe(query.as_str()))
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
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
