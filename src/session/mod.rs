use crate::tts::Voice;
use poise::serenity_prelude::UserId;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

pub mod actor;
pub mod driver;
pub mod manager;
mod startup_buffer;

#[derive(Debug, Clone)]
pub struct Speaker {
    pub(crate) user_id: UserId,
    pub(crate) name: String,
}

impl Speaker {
    pub fn new(user_id: UserId, name: String) -> Self {
        Self { user_id, name }
    }
}

#[derive(Clone)]
pub enum SessionControl {
    Stop,
    Leave, // user intentionally disconnected by command
    Disconnected,
}

#[derive(Clone)]
pub struct UserUtterance {
    pub text: String,
    pub voice: Arc<dyn Voice>,
    pub speaker: Speaker,
}

#[derive(Clone)]
pub struct Announcement {
    pub text: String,
    pub voice: Arc<dyn Voice>,
}

#[derive(Debug, Clone)]
pub struct SessionHandle {
    control_tx: mpsc::Sender<SessionControl>,
    user_data_tx: broadcast::Sender<UserUtterance>,
    announce_tx: mpsc::Sender<Announcement>,
}

impl SessionHandle {
    fn new(
        control_tx: mpsc::Sender<SessionControl>,
        user_data_tx: broadcast::Sender<UserUtterance>,
        announce_tx: mpsc::Sender<Announcement>,
    ) -> Self {
        Self {
            control_tx,
            user_data_tx,
            announce_tx,
        }
    }

    pub async fn speak(
        &self,
        text: String,
        voice: Arc<dyn Voice>,
        speaker: Speaker,
    ) -> anyhow::Result<()> {
        self.user_data_tx
            .send(UserUtterance {
                text,
                voice,
                speaker,
            })
            .map_err(|_| anyhow::anyhow!("session actor is not listening"))?;
        Ok(())
    }

    pub async fn announce(&self, text: String, voice: Arc<dyn Voice>) -> anyhow::Result<()> {
        self.announce_tx.send(Announcement { text, voice }).await?;
        Ok(())
    }

    pub async fn stop(&self) -> anyhow::Result<()> {
        self.control_tx.send(SessionControl::Stop).await?;
        Ok(())
    }

    pub async fn leave(&self) -> anyhow::Result<()> {
        self.control_tx.send(SessionControl::Leave).await?;
        Ok(())
    }
}
