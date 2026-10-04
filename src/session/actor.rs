use crate::session::driver::AudioDriver;
use crate::session::{Announcement, SessionControl, SessionHandle, UserUtterance};
use crate::tts::Voice;
use poise::serenity_prelude::UserId;
use std::sync::Arc;
use tokio::select;
use tokio::sync::{broadcast, mpsc};
use tracing::Instrument;

struct Utterance {
    text: String,
    speaker_announcement: Option<String>,
    voice: Arc<dyn Voice>,
}

enum IncomingUtterance {
    User(UserUtterance),
    Announcement(Announcement),
}

pub struct SessionActor {
    control_rx: mpsc::Receiver<SessionControl>,
    announce_rx: mpsc::Receiver<Announcement>,
    user_rx: broadcast::Receiver<UserUtterance>,
    driver: Arc<dyn AudioDriver>,
}

impl SessionActor {
    pub fn new(driver: Arc<dyn AudioDriver>) -> (Self, SessionHandle) {
        let (control_tx, control_rx) = mpsc::channel(100);
        let (user_data_tx, user_data_rx) = broadcast::channel(100);
        let (announce_tx, announce_rx) = mpsc::channel(100);

        {
            let driver = driver.clone();
            let control_tx = control_tx.clone();
            tokio::spawn(async move {
                driver.subscribe_to_disconnect_event(control_tx).await;
            });
        }

        let actor = Self {
            control_rx,
            announce_rx,
            user_rx: user_data_rx,
            driver,
        };

        (
            actor,
            SessionHandle::new(control_tx, user_data_tx, announce_tx),
        )
    }

    pub async fn run(mut self) {
        tracing::info!("Session actor started");

        const INITIAL_TOKEN: usize = 3;
        let mut tokens: isize = INITIAL_TOKEN as isize;
        let (playback_done_tx, mut playback_done_rx) = mpsc::channel::<()>(INITIAL_TOKEN * 2);
        let mut last_speaker_id: Option<UserId> = None;

        loop {
            let user_can_consume = tokens > 0;
            let event = select! {
                biased;
                Some(control) = self.control_rx.recv() => {
                    match control {
                        SessionControl::Stop => continue,
                        SessionControl::Leave => {
                            tracing::info!("Received Leave command");
                            break;
                        }
                        SessionControl::Disconnected => {
                            tracing::warn!("Driver disconnected unexpectedly");
                            break;
                        }
                    }
                }
                Some(()) = playback_done_rx.recv() => {
                    tokens += 1;
                    tracing::debug!(tokens_remaining = tokens, "Utterance token released");
                    continue;
                }
                Some(announcement) = self.announce_rx.recv() => IncomingUtterance::Announcement(announcement),
                result = self.user_rx.recv(), if user_can_consume => {
                    match result {
                        Ok(utterance) => IncomingUtterance::User(utterance),
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            tracing::warn!(skipped_utterances = count, "Session actor lagged");
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
                else => break,
            };

            let (text, voice, speaker) = match event {
                IncomingUtterance::User(utterance) => {
                    (utterance.text, utterance.voice, Some(utterance.speaker))
                }
                IncomingUtterance::Announcement(announcement) => {
                    (announcement.text, announcement.voice, None)
                }
            };
            tokens -= 1;
            let speaker_id = speaker.as_ref().map(|speaker| speaker.user_id);
            let speaker_announcement = speaker.and_then(|speaker| {
                if speaker_id != last_speaker_id {
                    last_speaker_id = speaker_id;
                    Some(speaker.name)
                } else {
                    None
                }
            });

            let utterance = Utterance {
                text,
                speaker_announcement,
                voice,
            };

            if Self::generate_and_play(utterance, self.driver.clone(), playback_done_tx.clone())
                .await
                .is_err()
            {
                tokens += 1;
            }
        }

        tracing::info!("Session actor stopping, cleaning up...");
        if let Err(e) = self.driver.leave().await {
            tracing::error!("Failed to leave voice channel during cleanup: {}", e);
        } else {
            tracing::info!("Successfully left voice channel.");
        }
    }

    async fn generate_and_play(
        utterance: Utterance,
        driver: Arc<dyn AudioDriver>,
        utterance_done: mpsc::Sender<()>,
    ) -> anyhow::Result<()> {
        let text_characters = utterance.text.chars().count()
            + utterance
                .speaker_announcement
                .as_ref()
                .map_or(0, |announcement| announcement.chars().count());
        let span = tracing::info_span!(
            "synthesize_utterance",
            voice_language = utterance.voice.language(),
            text_characters
        );

        let span = span.or_current();
        let result = async move {
            let mut outputs = Vec::new();
            let mut texts = Vec::new();
            if let Some(announcement) = utterance.speaker_announcement {
                texts.push(announcement);
            }
            texts.push(utterance.text);

            for text in texts {
                let audio_data = match utterance.voice.generate(&text).await {
                    Ok(data) => data,
                    Err(e) => return Err(anyhow::anyhow!(e).context("Failed to generate voice")),
                };
                outputs.push(audio_data);
            }

            driver.enqueue_outputs(outputs, utterance_done).await
        }
        .instrument(span.clone())
        .await;
        if let Err(error) = &result {
            span.in_scope(|| tracing::warn!(?error, "Couldn't generate playback"));
        }
        result
    }
}
