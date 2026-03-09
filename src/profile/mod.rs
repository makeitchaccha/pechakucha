use std::fmt::{Display, Formatter};

pub mod repository;
pub mod resolver;

#[derive(Debug, Clone, PartialEq)]
pub enum ProfileSource {
    UserOverride,
    GuildDefault,
    GlobalFallback,
}

impl Display for ProfileSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UserOverride => write!(f, "User Override"),
            Self::GuildDefault => write!(f, "Guild Default"),
            Self::GlobalFallback => write!(f, "Global Fallback"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub id: String,
    pub source: ProfileSource,
}

impl ResolvedProfile {
    pub fn user_override(id: String) -> Self {
        Self {
            id,
            source: ProfileSource::UserOverride,
        }
    }

    pub fn guild_default(id: String) -> Self {
        Self {
            id,
            source: ProfileSource::GuildDefault,
        }
    }

    pub fn global_fallback(id: String) -> Self {
        Self {
            id,
            source: ProfileSource::GlobalFallback,
        }
    }
}

#[cfg(test)]
mod test_utils {
    use crate::profile::repository::ProfileRepository;
    use async_trait::async_trait;
    use poise::serenity_prelude::{GuildId, UserId};
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    pub struct MockProfileRepository {
        user_profiles: Arc<Mutex<HashMap<UserId, String>>>,
        guild_profiles: Arc<Mutex<HashMap<GuildId, String>>>,
    }

    impl MockProfileRepository {
        pub fn new() -> Self {
            Self {
                user_profiles: Arc::new(Mutex::new(HashMap::new())),
                guild_profiles: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    #[async_trait]
    impl ProfileRepository for MockProfileRepository {
        async fn find_by_user(&self, user_id: UserId) -> anyhow::Result<Option<String>> {
            Ok(self.user_profiles.lock().await.get(&user_id).cloned())
        }

        async fn find_by_guild(&self, guild_id: GuildId) -> anyhow::Result<Option<String>> {
            Ok(self.guild_profiles.lock().await.get(&guild_id).cloned())
        }

        async fn save_user(&self, user_id: UserId, profile_id: &str) -> anyhow::Result<()> {
            self.user_profiles
                .lock()
                .await
                .insert(user_id, profile_id.to_owned());
            Ok(())
        }

        async fn save_guild(&self, guild_id: GuildId, profile_id: &str) -> anyhow::Result<()> {
            self.guild_profiles
                .lock()
                .await
                .insert(guild_id, profile_id.to_owned());
            Ok(())
        }

        async fn delete_user(&self, user_id: UserId) -> anyhow::Result<()> {
            self.user_profiles.lock().await.remove(&user_id);
            Ok(())
        }

        async fn delete_guild(&self, guild_id: GuildId) -> anyhow::Result<()> {
            self.guild_profiles.lock().await.remove(&guild_id);
            Ok(())
        }
    }
}
