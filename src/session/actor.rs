use crate::session::driver::AudioDriver;
use crate::session::{Priority, SessionCommand, SessionHandle, Speaker};
use crate::tts::Voice;
use poise::serenity_prelude::UserId;
use std::sync::Arc;
use tokio::select;
use tokio::sync::{broadcast, mpsc};
use tracing;

#[derive(Clone)]
enum WorkerCommand {
    GenerateAndPlay(GenerateAndPlay),
}

#[derive(Clone)]
struct GenerateAndPlay {
    text: String,
    speaker: Option<Speaker>,
    voice: Arc<dyn Voice>,
}

pub struct SessionActor {
    rx: mpsc::Receiver<SessionCommand>,
    system_tx: mpsc::Sender<WorkerCommand>,
    user_tx: broadcast::Sender<WorkerCommand>,
    driver: Arc<dyn AudioDriver>,
}

impl SessionActor {
    pub fn new(driver: Arc<dyn AudioDriver>) -> (Self, SessionHandle) {
        let (cmd_tx, cmd_rx) = mpsc::channel(100);

        let (system_tx, system_rx) = mpsc::channel(100);
        let (user_tx, user_rx) = broadcast::channel(100);

        {
            let driver = driver.clone();
            let cmd_tx = cmd_tx.clone();
            tokio::spawn(async move {
                driver.subscribe_to_disconnect_event(cmd_tx).await;
            });
        }

        tokio::spawn(Self::worker_loop(driver.clone(), system_rx, user_rx));
        let actor = Self {
            rx: cmd_rx,
            system_tx,
            user_tx,
            driver,
        };

        (actor, SessionHandle::new(cmd_tx))
    }

    pub async fn run(mut self) {
        tracing::info!("Session actor started");

        while let Some(cmd) = self.rx.recv().await {
            match cmd {
                SessionCommand::Speak {
                    text,
                    voice,
                    speaker,
                    priority,
                } => {
                    let command = WorkerCommand::GenerateAndPlay(GenerateAndPlay {
                        text,
                        speaker,
                        voice,
                    });

                    // ignore since not recoverable
                    match priority {
                        Priority::System => {
                            let _ = self.system_tx.send(command).await;
                        }
                        Priority::User => {
                            let _ = self.user_tx.send(command);
                        }
                    };
                }
                SessionCommand::Stop => {}
                SessionCommand::Leave => {
                    tracing::info!("Received Leave command");
                    break;
                }
                SessionCommand::Disconnect => {
                    tracing::warn!("Driver disconnected unexpectedly");
                    break;
                }
            }
        }

        tracing::info!("Session actor stopping, cleaning up...");

        if let Err(e) = self.driver.leave().await {
            tracing::error!("Failed to leave voice channel during cleanup: {}", e);
        } else {
            tracing::info!("Successfully left voice channel.");
        }
    }

    async fn worker_loop(
        driver: Arc<dyn AudioDriver>,
        mut system_rx: mpsc::Receiver<WorkerCommand>,
        mut user_rx: broadcast::Receiver<WorkerCommand>,
    ) {
        tracing::info!("Worker started");

        // Token system for eager voice generation
        // user voice generation is throttled with tokens
        const INITIAL_TOKEN: usize = 3;
        let mut tokens: isize = INITIAL_TOKEN as isize;

        let mut songbird_rx = {
            let (tx, rx) = mpsc::channel(INITIAL_TOKEN * 2);
            driver.subscribe_to_end_event(tx).await;
            rx
        };

        let mut last_speaker_id: Option<UserId> = None;

        loop {
            let user_can_consume = tokens > 0;

            select! {
                biased;
                Some(_) = songbird_rx.recv() => {
                    if tokens < INITIAL_TOKEN as isize {
                        tokens += 1;
                        tracing::debug!("Token released. Current: {}", tokens);
                    }
                }
                Some(cmd) = system_rx.recv() => {
                    match cmd {
                        WorkerCommand::GenerateAndPlay(cmd) => {
                            let mut segments = Vec::new();
                            let current_speaker = cmd.speaker.as_ref().map(|s| s.user_id);

                            // read name when current speaker is not same as last one.
                            if current_speaker != last_speaker_id && let Some(speaker) = cmd.speaker {
                                last_speaker_id = current_speaker;
                                segments.push(speaker.name);
                            }
                            segments.push(cmd.text.clone());

                            match Self::generate_and_play(segments, cmd.voice, driver.clone()).await {

                                Ok(len) => {
                                    tracing::debug!("consuming {} tokens", len);
                                    tokens -= len as isize;
                                }
                                Err(err) => {
                                    tracing::warn!("Couldn't generate playback: {:?}", err);
                                }
                            }
                        },
                    }
                }

                cmd_result = user_rx.recv(), if user_can_consume => {
                    match cmd_result {
                        Ok(WorkerCommand::GenerateAndPlay(cmd)) => {
                            let mut segments = Vec::new();
                            let current_speaker = cmd.speaker.as_ref().map(|s| s.user_id);

                            // read name when current speaker is not same as last one.
                            if current_speaker != last_speaker_id && let Some(speaker) = cmd.speaker {
                                last_speaker_id = current_speaker;
                                segments.push(speaker.name);
                            }
                            segments.push(cmd.text.clone());

                            match Self::generate_and_play(segments, cmd.voice, driver.clone()).await {
                                Ok(len) => {
                                    tracing::debug!("consuming {} tokens", len);
                                    tokens -= len as isize;
                                }
                                Err(err) => {
                                    tracing::warn!("Couldn't generate playback: {:?}", err);
                                }
                            }
                        },
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            tracing::warn!("worker lagged, skip {} commands", count);
                            continue;
                        },
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::info!("worker closed");
                            break;
                        }
                    }
                }
                else => {
                    break;
                }
            }
        }
    }

    async fn generate_and_play(
        segment: Vec<String>,
        voice: Arc<dyn Voice>,
        driver: Arc<dyn AudioDriver>,
    ) -> anyhow::Result<usize> {
        let mut audios = Vec::new();
        for segment in segment.iter() {
            let audio_data = match voice.generate(segment).await {
                Ok(data) => data,
                Err(e) => {
                    return Err(anyhow::anyhow!(e).context("Failed to generate voice"));
                }
            };
            audios.push(audio_data);
        }

        let len = audios.len();
        driver.enqueue(audios).await;

        Ok(len)
    }
}
