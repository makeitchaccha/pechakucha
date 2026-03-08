use crate::tts::google_cloud::GoogleCloudVoiceConfig;
use anyhow::anyhow;
use config::{Config, File};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;
use crate::tts::VoiceDetail;

pub fn load_config(path: &Path) -> anyhow::Result<AppConfig> {
    let config = Config::builder()
        .add_source(File::from(path))
        .add_source(config::Environment::with_prefix("TTSBOT").separator("_"))
        .build()?;

    config.try_deserialize()
        .map_err(|e| anyhow!(e))
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub bot: BotConfig,

    pub database: DatabaseConfig,

    #[serde(default)]
    pub backend: BackendConfig,

    pub cache: CacheConfig,

    pub profiles: HashMap<String, ProfileConfig>,
}

impl AppConfig {
    pub fn verify(&self) -> anyhow::Result<()> {
        if self.bot.token.is_empty() {
            return Err(anyhow!("bot token is empty"))
        }

        if !self.profiles.contains_key(&self.bot.global_profile) {
            return Err(anyhow!("No profile matched for {}, specified for global_profile", &self.bot.global_profile))
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotConfig {
    pub token: String,
    pub global_profile: String,
}

#[derive(Debug, Clone, Deserialize)]
pub enum DatabaseKind{
    #[serde(rename = "postgres")]
    Postgres,
    #[serde(rename = "sqlite")]
    SQLite,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    pub kind: DatabaseKind,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct BackendConfig {
    pub google_cloud: Option<GoogleCloudBackendConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GoogleCloudBackendConfig {
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 { 5 }

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum CacheConfig {
    #[serde(rename = "disabled")]
    Disabled,
    #[serde(rename = "in_memory")]
    InMemory(InMemoryCacheConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct InMemoryCacheConfig {
    pub capacity: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProfileConfig {
    pub note: Option<VoiceDetailConfig>,

    #[serde(flatten)]
    pub voice_backend: ProfileBackendConfig,
}

#[derive(Clone, Debug, Deserialize, Default)]
pub struct VoiceDetailConfig {
    pub name: Option<String>,

    pub description: Option<String>,
}

impl VoiceDetailConfig {
    /// Resolve this config into VoiceDetail using a default
    ///
    /// Use configured value if present, otherwise fallback to default.
    pub fn resolve(&self, default: VoiceDetail) -> VoiceDetail {
        VoiceDetail {
            name: self.name.clone().unwrap_or(default.name),
            provider: default.provider,
            description: self.description.clone().or(default.description),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "backend")]
pub enum ProfileBackendConfig {
    #[serde(rename = "google_cloud")]
    GoogleCloudVoice(GoogleCloudVoiceConfig),
}

impl ProfileBackendConfig {
    pub fn generate_default_detail(&self, name: &str) -> VoiceDetail {
        match &self {
            ProfileBackendConfig::GoogleCloudVoice(config) => config.generate_default_detail(name)
        }
    }
}