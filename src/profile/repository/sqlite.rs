use crate::profile::ResolvedProfile;
use crate::profile::repository::ProfileRepository;
use async_trait::async_trait;
use poise::serenity_prelude::{GuildId, UserId};
use sqlx::SqlitePool;

pub struct SQLiteProfileRepository {
    pool: SqlitePool,
}

impl SQLiteProfileRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProfileRepository for SQLiteProfileRepository {
    async fn find_by_user(&self, user_id: UserId) -> anyhow::Result<Option<String>> {
        let id = user_id.to_string();
        let record = sqlx::query!(
            "SELECT profile_id FROM user_profiles WHERE user_id = ? -- sqlite",
            id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(record.map(|record| record.profile_id))
    }

    async fn find_by_guild(&self, guild_id: GuildId) -> anyhow::Result<Option<String>> {
        let id = guild_id.to_string();
        let record = sqlx::query!(
            "SELECT profile_id FROM guild_profiles WHERE guild_id = ? -- sqlite",
            id
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(record.map(|record| record.profile_id))
    }

    async fn find_highest_priority(
        &self,
        user_id: UserId,
        guild_id: GuildId,
    ) -> anyhow::Result<Option<ResolvedProfile>> {
        let user_id = user_id.to_string();
        let guild_id = guild_id.to_string();
        let record = sqlx::query!(
            r#"
                SELECT 0 AS "source_id!: i64", profile_id FROM user_profiles WHERE user_id = ?
                UNION ALL
                SELECT 1 AS "source_id!: i64", profile_id FROM guild_profiles WHERE guild_id = ?
                ORDER BY "source_id!: i64" ASC -- 0 (User) takes precedence over 1 (Guild)
                LIMIT 1 -- sqlite"#,
            user_id,
            guild_id
        )
        .fetch_optional(&self.pool)
        .await?;

        match record {
            None => Ok(None),
            Some(record) => {
                let source_id = record.source_id;
                let profile_id = record.profile_id;
                match source_id {
                    0 => Ok(Some(ResolvedProfile::user_override(profile_id))),
                    1 => Ok(Some(ResolvedProfile::guild_default(profile_id))),
                    _ => Err(anyhow::anyhow!("no profile found")),
                }
            }
        }
    }

    async fn save_user(&self, user_id: UserId, profile_id: &str) -> anyhow::Result<()> {
        let id = user_id.to_string();
        let _ = sqlx::query!(
                "INSERT INTO user_profiles(user_id, profile_id) VALUES(?, ?) ON CONFLICT (user_id) DO UPDATE SET profile_id = EXCLUDED.profile_id -- sqlite",
                id,
                profile_id
            ).execute(&self.pool).await?;

        Ok(())
    }

    async fn save_guild(&self, guild_id: GuildId, profile_id: &str) -> anyhow::Result<()> {
        let id = guild_id.to_string();
        let _ = sqlx::query!(
                "INSERT INTO guild_profiles(guild_id, profile_id) VALUES(?, ?) ON CONFLICT (guild_id) DO UPDATE SET profile_id = EXCLUDED.profile_id -- sqlite",
                id,
                profile_id
            ).execute(&self.pool).await?;

        Ok(())
    }

    async fn delete_user(&self, user_id: UserId) -> anyhow::Result<()> {
        let id = user_id.to_string();
        let _ = sqlx::query!("DELETE FROM user_profiles WHERE user_id = ? -- sqlite", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn delete_guild(&self, guild_id: GuildId) -> anyhow::Result<()> {
        let id = guild_id.to_string();
        let _ = sqlx::query!(
            "DELETE FROM guild_profiles WHERE guild_id = ? -- sqlite",
            id
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
