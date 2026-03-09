#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;

use crate::profile::ResolvedProfile;
use anyhow::Result;
use async_trait::async_trait;
use poise::serenity_prelude::{GuildId, UserId};

#[async_trait]
pub trait ProfileRepository: Send + Sync {
    async fn find_by_user(&self, user_id: UserId) -> Result<Option<String>>;
    async fn find_by_guild(&self, guild_id: GuildId) -> Result<Option<String>>;

    /// for document purpose implementation for find profile id
    /// concrete repository may implement more efficient procedures
    /// such as oneliner query for priority order.
    async fn find_highest_priority(
        &self,
        user_id: UserId,
        guild_id: GuildId,
    ) -> Result<Option<ResolvedProfile>> {
        // user first
        if let Some(profile_id) = self.find_by_user(user_id).await? {
            return Ok(Some(ResolvedProfile::user_override(profile_id)));
        }

        // then guild
        if let Some(profile_id) = self.find_by_guild(guild_id).await? {
            return Ok(Some(ResolvedProfile::guild_default(profile_id)));
        }

        Ok(None)
    }

    async fn save_user(&self, user_id: UserId, profile_id: &str) -> Result<()>;
    async fn save_guild(&self, guild_id: GuildId, profile_id: &str) -> Result<()>;

    async fn delete_user(&self, user_id: UserId) -> Result<()>;
    async fn delete_guild(&self, guild_id: GuildId) -> Result<()>;
}
