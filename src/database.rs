use anyhow::Context;
use sqlx::migrate::{AppliedMigration, Migrate, Migration};
use std::collections::HashSet;
use std::sync::Arc;
use text_to_speech_rs::profile::repository::ProfileRepository;

pub enum WrappedPool {
    Sqlite(sqlx::Pool<sqlx::Sqlite>),
    Postgres(sqlx::Pool<sqlx::Postgres>),
}

impl WrappedPool {
    pub async fn migrate_up(&self) -> anyhow::Result<()> {
        match &self {
            WrappedPool::Sqlite(pool) => {
                sqlx::migrate!("./migrations/sqlite")
                    .run(pool)
                    .await
                    .context("Failed to run SQLite migrations")?;
                Ok(())
            }
            WrappedPool::Postgres(pool) => {
                sqlx::migrate!("./migrations/postgres")
                    .run(pool)
                    .await
                    .context("Failed to run PostgreSQL migrations")?;
                Ok(())
            }
        }
    }

    async fn collect_migration_status(
        applied_migration: Vec<AppliedMigration>,
        migrator: &sqlx::migrate::Migrator,
    ) -> anyhow::Result<Vec<(Migration, bool)>> {
        let applied_versions: HashSet<i64> =
            applied_migration.into_iter().map(|m| m.version).collect();

        Ok(migrator
            .iter()
            .map(|migration| {
                let is_applied = applied_versions.contains(&migration.version);
                (migration.clone(), is_applied)
            })
            .collect())
    }

    pub async fn migrate_status(&self) -> anyhow::Result<Vec<(Migration, bool)>> {
        match &self {
            WrappedPool::Sqlite(pool) => {
                let migrator = sqlx::migrate!("./migrations/sqlite");
                let mut conn = pool.acquire().await?;
                Self::collect_migration_status(conn.list_applied_migrations().await?, &migrator)
                    .await
            }
            WrappedPool::Postgres(pool) => {
                let migrator = sqlx::migrate!("./migrations/postgres");
                let mut conn = pool.acquire().await?;
                Self::collect_migration_status(conn.list_applied_migrations().await?, &migrator)
                    .await
            }
        }
    }

    pub fn profile_repository(&self) -> Arc<dyn ProfileRepository> {
        match &self {
            WrappedPool::Sqlite(pool) => {
                #[cfg(feature = "sqlite")]
                {
                    use text_to_speech_rs::profile::repository::sqlite::SQLiteProfileRepository;
                    Arc::new(SQLiteProfileRepository::new(pool.clone()))
                }
                #[cfg(not(feature = "sqlite"))]
                unreachable!("sqlite feature must be enabled to create this pool")
            }
            WrappedPool::Postgres(pool) => {
                #[cfg(feature = "postgres")]
                {
                    use text_to_speech_rs::profile::repository::postgres::PostgresRepository;
                    Arc::new(PostgresRepository::new(pool.clone()))
                }
                #[cfg(not(feature = "postgres"))]
                unreachable!("postgres feature must be enabled to create this pool")
            }
        }
    }
}
