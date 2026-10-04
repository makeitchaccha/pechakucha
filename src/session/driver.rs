use crate::session::SessionControl;
use crate::session::startup_buffer::StartupBuffer;
use crate::tts::AudioOutput;
use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use songbird::input::{AsyncAdapterStream, AsyncReadOnlySource, AudioStream, Input, LiveInput};
use songbird::{Call, CoreEvent, Event, EventContext, EventHandler, TrackEvent};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio_util::io::StreamReader;
use tracing::Instrument;

const VOICEVOX_PCM_BYTES_PER_SECOND: usize = 48_000;
const VOICEVOX_RING_BUFFER_SECS: usize = 20;

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
        let mut inputs = Vec::with_capacity(outputs.len());
        for output in outputs {
            let input = match output {
                AudioOutput::Buffered(bytes) => bytes.to_vec().into(),
                AudioOutput::Stream { chunks, timing } => {
                    let span = timing.span.clone();
                    let buffer_future = StartupBuffer::new(chunks, timing).run();
                    let (buffered_chunks, chunks) = buffer_future.instrument(span).await;
                    let stream = stream::iter(buffered_chunks)
                        .chain(stream::unfold(chunks, |mut receiver| async move {
                            receiver.recv().await.map(|chunk| (chunk, receiver))
                        }))
                        .map(|result| result.map_err(std::io::Error::other));
                    let reader = StreamReader::new(Box::pin(stream));
                    let source = AsyncReadOnlySource::new(reader);
                    let ring_buffer = VOICEVOX_PCM_BYTES_PER_SECOND * VOICEVOX_RING_BUFFER_SECS;
                    Input::Live(
                        LiveInput::Raw(AudioStream {
                            input: Box::new(AsyncAdapterStream::new(Box::new(source), ring_buffer)),
                        }),
                        None,
                    )
                }
            };
            inputs.push(input);
        }

        let mut call = self.call.lock().await;
        let last_index = inputs.len().saturating_sub(1);
        for (index, input) in inputs.into_iter().enumerate() {
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
