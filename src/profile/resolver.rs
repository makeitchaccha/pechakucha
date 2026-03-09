use crate::profile::ResolvedProfile;
use crate::profile::repository::ProfileRepository;
use anyhow::Result;
use poise::serenity_prelude::{GuildId, UserId};
use std::sync::Arc;

pub struct ProfileResolver {
    repository: Arc<dyn ProfileRepository>,
    fallback_profile_id: String,
}

impl ProfileResolver {
    pub fn new(repository: Arc<dyn ProfileRepository>, fallback_profile_id: String) -> Self {
        Self {
            repository,
            fallback_profile_id,
        }
    }

    pub async fn resolve_with_fallback(
        &self,
        user_id: UserId,
        guild_id: GuildId,
    ) -> Result<ResolvedProfile> {
        if let Some(profile_id) = self
            .repository
            .find_highest_priority(user_id, guild_id)
            .await?
        {
            return Ok(profile_id);
        }

        Ok(ResolvedProfile::global_fallback(
            self.fallback_profile_id.clone(),
        ))
    }

    pub async fn resolve_guild_with_fallback(&self, guild_id: GuildId) -> Result<ResolvedProfile> {
        if let Some(profile_id) = self.repository.find_by_guild(guild_id).await? {
            return Ok(ResolvedProfile::guild_default(profile_id));
        }

        Ok(ResolvedProfile::global_fallback(
            self.fallback_profile_id.clone(),
        ))
    }

    pub fn fallback(&self) -> &str {
        &self.fallback_profile_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::ProfileSource;
    use crate::profile::test_utils::MockProfileRepository;

    struct TestContext {
        resolver: ProfileResolver,
        repository: Arc<dyn ProfileRepository>,
    }

    impl TestContext {
        fn new() -> Self {
            let repository = Arc::new(MockProfileRepository::new());
            let resolver = ProfileResolver::new(repository.clone(), "fallback-profile".to_string());
            Self {
                resolver,
                repository,
            }
        }

        async fn with_user_profile(self, user_id: u64, profile: &str) -> Self {
            self.repository
                .save_user(UserId::from(user_id), profile)
                .await
                .unwrap();
            self
        }

        async fn with_guild_profile(self, guild_id: u64, profile: &str) -> Self {
            self.repository
                .save_guild(GuildId::from(guild_id), profile)
                .await
                .unwrap();
            self
        }
    }

    #[tokio::test]
    async fn user_profile_takes_precedence_over_guild_profile() {
        let ctx = TestContext::new()
            .with_user_profile(1, "user-P")
            .await
            .with_guild_profile(1, "guild-P")
            .await;

        let result = ctx
            .resolver
            .resolve_with_fallback(UserId::from(1), GuildId::from(1))
            .await
            .unwrap();

        assert_eq!(result.source, ProfileSource::UserOverride);
        assert_eq!(result.id, "user-P");
    }

    #[tokio::test]
    async fn uses_guild_profile_if_user_profile_is_missing() {
        let ctx = TestContext::new().with_guild_profile(1, "guild-P").await;

        let result = ctx
            .resolver
            .resolve_with_fallback(UserId::from(1), GuildId::from(1))
            .await
            .unwrap();

        assert_eq!(result.source, ProfileSource::GuildDefault);
        assert_eq!(result.id, "guild-P");
    }

    #[tokio::test]
    async fn returns_fallback_if_nothing_configured() {
        let ctx = TestContext::new();

        let result = ctx
            .resolver
            .resolve_with_fallback(UserId::from(1), GuildId::from(1))
            .await
            .unwrap();

        assert_eq!(result.source, ProfileSource::GlobalFallback);
        assert_eq!(result.id, "fallback-profile");
    }

    #[tokio::test]
    async fn guild_profile_takes_precedence_over_fallback() {
        let ctx = TestContext::new().with_guild_profile(1, "guild-P").await;

        let result = ctx
            .resolver
            .resolve_guild_with_fallback(GuildId::from(1))
            .await
            .unwrap();

        assert_eq!(result.source, ProfileSource::GuildDefault);
        assert_eq!(result.id, "guild-P");
    }

    #[tokio::test]
    async fn guild_returns_fallback_if_nothing_configured() {
        let ctx = TestContext::new();

        let result = ctx
            .resolver
            .resolve_guild_with_fallback(GuildId::from(1))
            .await
            .unwrap();

        assert_eq!(result.source, ProfileSource::GlobalFallback);
        assert_eq!(result.id, "fallback-profile");
    }
}
