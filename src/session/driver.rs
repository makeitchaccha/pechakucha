use crate::session::SessionCommand;
use async_trait::async_trait;
use songbird::{Call, CoreEvent, Event, EventContext, EventHandler, TrackEvent};
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};

#[async_trait]
pub trait AudioDriver: Sync + Send {
    async fn enqueue(&self, audios: Vec<Vec<u8>>);

    async fn leave(&self) -> anyhow::Result<()>;

    async fn subscribe_to_end_event(&self, tx: mpsc::Sender<()>);

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionCommand>);
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
    async fn enqueue(&self, data: Vec<Vec<u8>>) {
        let mut call = self.call.lock().await;
        for audio in data {
            call.enqueue_input(audio.into()).await;
        }
    }

    async fn leave(&self) -> anyhow::Result<()> {
        let mut call = self.call.lock().await;
        call.leave().await?;
        Ok(())
    }

    async fn subscribe_to_end_event(&self, tx: mpsc::Sender<()>) {
        let mut call = self.call.lock().await;
        call.add_global_event(
            Event::Track(TrackEvent::End),
            SongbirdEventHandler { tx, result: () },
        );
    }

    async fn subscribe_to_disconnect_event(&self, tx: mpsc::Sender<SessionCommand>) {
        let mut call = self.call.lock().await;
        call.add_global_event(
            Event::Core(CoreEvent::DriverDisconnect),
            SongbirdEventHandler {
                tx,
                result: SessionCommand::Disconnect,
            },
        );
    }
}
