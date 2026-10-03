use crate::tts::{AudioOutput, Voice, VoiceError};
use async_trait::async_trait;
use moka::future::Cache;
use sha2::Digest;
use sha2::digest::Update;
use tokio::sync::mpsc;

pub struct CachedVoice {
    identifier: String,
    inner: Box<dyn Voice>,
    cache: Cache<String, Vec<u8>>,
}

impl CachedVoice {
    pub fn new(inner: Box<dyn Voice>, cache: Cache<String, Vec<u8>>) -> Self {
        Self {
            identifier: format!("cached-{}", inner.identifier()),
            inner,
            cache,
        }
    }
}

#[async_trait]
impl Voice for CachedVoice {
    fn identifier(&self) -> &str {
        &self.identifier
    }

    fn language(&self) -> &str {
        self.inner.language()
    }

    async fn generate(&self, text: &str) -> Result<AudioOutput, VoiceError> {
        tracing::debug!("cached-voice requested to generate: {}", text);
        let key = hex::encode(
            sha2::Sha256::new()
                .chain(self.identifier.as_bytes())
                .chain(text.as_bytes())
                .finalize(),
        );

        if let Some(data) = self.cache.get(&key).await {
            tracing::debug!("cache hit for {} with key {}", &text, &key);
            return Ok(AudioOutput::Buffered(data.into()));
        }

        tracing::debug!(
            "cache miss for {} with key {}, delegate request",
            &text,
            &key
        );
        match self.inner.generate(text).await? {
            AudioOutput::Buffered(data) => {
                self.cache.insert(key, data.to_vec()).await;
                Ok(AudioOutput::Buffered(data))
            }
            AudioOutput::Stream { mut chunks, timing } => {
                let (tx, rx) = mpsc::channel(8);
                let cache = self.cache.clone();
                tokio::spawn(async move {
                    let mut cached_audio = Vec::new();
                    while let Some(chunk) = chunks.recv().await {
                        match chunk {
                            Ok(bytes) => {
                                cached_audio.extend_from_slice(&bytes);
                                if tx.send(Ok(bytes)).await.is_err() {
                                    return;
                                }
                            }
                            Err(error) => {
                                let _ = tx.send(Err(error)).await;
                                return;
                            }
                        }
                    }
                    cache.insert(key, cached_audio).await;
                });
                Ok(AudioOutput::Stream { chunks: rx, timing })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::test_utils::MockVoice;

    #[tokio::test]
    async fn test_cache_hit() {
        let mock = MockVoice::new();

        let cached_voice = CachedVoice::new(Box::new(mock.clone()), Cache::new(100));

        let text = "hello";

        // in case of same text
        let result = cached_voice.generate(text).await.unwrap();
        assert_eq!(result.into_bytes().await.unwrap().as_ref(), b"hello");
        assert_eq!(
            mock.call_count(),
            1,
            "First call should hit the inner voice"
        );

        let result = cached_voice.generate(text).await.unwrap();
        assert_eq!(result.into_bytes().await.unwrap().as_ref(), b"hello");
        assert_eq!(mock.call_count(), 1, "Second call should hit the cache");

        // different text
        let _ = cached_voice.generate("world").await;
        assert_eq!(mock.call_count(), 2, "New text should hit the inner voice");
    }
}
