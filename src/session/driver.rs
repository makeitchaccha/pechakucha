use crate::session::SessionControl;
use crate::tts::AudioOutput;
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use songbird::input::{AsyncAdapterStream, AsyncReadOnlySource, AudioStream, Input, LiveInput};
use songbird::{Call, CoreEvent, Event, EventContext, EventHandler, TrackEvent};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio_util::io::StreamReader;

#[async_trait]
pub trait AudioDriver: Sync + Send {
    async fn enqueue(
        &self,
        audios: Vec<Vec<u8>>,
        utterance_done: mpsc::Sender<()>,
    ) -> anyhow::Result<()>;

    async fn enqueue_outputs(
        &self,
        outputs: Vec<AudioOutput>,
        utterance_done: mpsc::Sender<()>,
    ) -> anyhow::Result<()>;

    async fn leave(&self) -> anyhow::Result<()>;

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionControl>);
}

pub struct SongbirdDriver {
    pub call: Arc<Mutex<Call>>,
}

struct SongbirdEventHandler<T: Send + Sync + Clone> {
    tx: mpsc::Sender<T>,
    result: T,
}
#[async_trait]
impl<T: Send + Sync + Clone> EventHandler for SongbirdEventHandler<T> {
    async fn act(&self, _: &EventContext<'_>) -> Option<Event> {
        let _ = self.tx.send(self.result.clone()).await;
        None
    }
}

#[async_trait]
impl AudioDriver for SongbirdDriver {
    async fn enqueue(
        &self,
        data: Vec<Vec<u8>>,
        utterance_done: mpsc::Sender<()>,
    ) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        let last_index = data.len().saturating_sub(1);
        for (index, audio) in data.into_iter().enumerate() {
            let track = call.enqueue_input(audio.into()).await;
            if index == last_index {
                track
                    .add_event(
                        Event::Track(TrackEvent::End),
                        SongbirdEventHandler {
                            tx: utterance_done.clone(),
                            result: (),
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to subscribe to utterance end: {e}"))?;
            }
        }
        Ok(())
    }

    async fn enqueue_outputs(
        &self,
        outputs: Vec<AudioOutput>,
        utterance_done: mpsc::Sender<()>,
    ) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        let last_index = outputs.len().saturating_sub(1);
        for (index, output) in outputs.into_iter().enumerate() {
            let (input, timing) = match output {
                AudioOutput::Buffered(bytes) => (bytes.to_vec().into(), None),
                AudioOutput::Stream { chunks, timing } => {
                    let stream = stream::unfold(chunks, |mut receiver| async move {
                        receiver.recv().await.map(|chunk| (chunk, receiver))
                    })
                    .map(|result| result.map_err(std::io::Error::other));
                    let reader = StreamReader::new(Box::pin(stream));
                    let source = AsyncReadOnlySource::new(reader);
                    let ring_buffer = timing
                        .max_chunk_arrival
                        .or(timing.chunk_audio_duration)
                        .map(|duration| {
                            let safe_duration = duration.mul_f64(1.5);
                            // WAV PCM16 is 24 kHz mono, plus its header.
                            (safe_duration.as_secs_f64() * 24_000.0 * 2.0) as usize + 44
                        })
                        .unwrap_or(64 * 1024);
                    let adapter = AsyncAdapterStream::new(Box::new(source), ring_buffer);
                    let input = Input::Live(
                        LiveInput::Raw(AudioStream {
                            input: Box::new(adapter),
                        }),
                        None,
                    );
                    (input, Some(timing))
                }
            };

            if let Some(timing) = timing {
                let safety_factor = 1.5_f64;
                let suggested_buffer = timing
                    .max_chunk_arrival
                    .or(timing.chunk_audio_duration)
                    .map(|duration| duration.mul_f64(safety_factor));
                tracing::debug!(
                    ?timing,
                    ?suggested_buffer,
                    safety_factor,
                    "Calculated playback start buffer from voice timing profile"
                );
            }
            let track = call.enqueue_input(input).await;
            if index == last_index {
                track
                    .add_event(
                        Event::Track(TrackEvent::End),
                        SongbirdEventHandler {
                            tx: utterance_done.clone(),
                            result: (),
                        },
                    )
                    .map_err(|e| anyhow::anyhow!("Failed to subscribe to utterance end: {e}"))?;
            }
        }
        Ok(())
    }

    async fn leave(&self) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        call.leave().await?;
        Ok(())
    }

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionControl>) {
        let mut call = self.call.lock().await;
        call.add_global_event(
            Event::Core(CoreEvent::DriverDisconnect),
            SongbirdEventHandler {
                tx,
                result: SessionControl::Disconnected,
            },
        );
    }
}
