use crate::session::SessionHandle;
use anyhow::anyhow;
use dashmap::DashMap;
use poise::serenity_prelude::{ChannelId, GuildId};

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub handle: SessionHandle,
    pub text_channel: ChannelId,
    pub voice_channel: ChannelId,
}

#[derive(Debug)]
pub struct SessionManager {
    sessions: DashMap<GuildId, SessionInfo>,

    text_channels: DashMap<ChannelId, GuildId>,
    voice_channels: DashMap<ChannelId, GuildId>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            text_channels: DashMap::new(),
            voice_channels: DashMap::new(),
        }
    }

    pub fn register(
        &self,
        guild_id: GuildId,
        text_channel: ChannelId,
        voice_channel: ChannelId,
        handle: SessionHandle,
    ) {
        self.sessions.insert(
            guild_id,
            SessionInfo {
                handle,
                text_channel,
                voice_channel,
            },
        );
        self.text_channels.insert(text_channel, guild_id);
        self.voice_channels.insert(voice_channel, guild_id);
        tracing::info!(
            "Registered session for guild: {}, text_channel: {}",
            guild_id,
            text_channel
        );
    }

    pub fn get(&self, guild_id: GuildId) -> Option<SessionInfo> {
        self.sessions.get(&guild_id).map(|r| r.value().clone())
    }

    pub fn get_by_text_channel(&self, text_channel: ChannelId) -> Option<SessionInfo> {
        let guild_id = self.text_channels.get(&text_channel)?;
        self.get(*guild_id)
    }

    pub fn get_by_voice_channel(&self, voice_channel: ChannelId) -> Option<SessionInfo> {
        let guild_id = self.voice_channels.get(&voice_channel)?;
        self.get(*guild_id)
    }

    pub fn update_voice_channel(
        &self,
        old: ChannelId,
        new: ChannelId,
    ) -> Result<(), anyhow::Error> {
        let guild_id = self
            .voice_channels
            .get(&old)
            .ok_or(anyhow!("Guild ID not found for voice channel {}", old))?
            .to_owned();

        let mut session_entry = self
            .sessions
            .get_mut(&guild_id)
            .ok_or(anyhow!("Session info not found for guild {}", guild_id))?;

        session_entry.voice_channel = new;
        self.voice_channels.remove(&old);
        self.voice_channels.insert(new, guild_id);

        Ok(())
    }

    pub fn remove(&self, guild_id: GuildId) {
        if let Some((_, session_info)) = self.sessions.remove(&guild_id) {
            if self
                .text_channels
                .remove(&session_info.text_channel)
                .is_none()
            {
                tracing::warn!(
                    "Inconsistency: Text channel index was missing for guild {}",
                    guild_id
                );
            }

            if self
                .voice_channels
                .remove(&session_info.voice_channel)
                .is_none()
            {
                tracing::warn!(
                    "Inconsistency: Voice channel index was missing for guild {}",
                    guild_id
                );
            }

            tracing::info!("Removed session for guild: {}", guild_id);
        }
    }
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}
