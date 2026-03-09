use crate::tts::{Voice, VoiceError};
use async_trait::async_trait;
use moka::future::Cache;
use sha2::Digest;
use sha2::digest::Update;

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

    async fn generate(&self, text: &str) -> Result<Vec<u8>, VoiceError> {
        tracing::debug!("cached-voice requested to generate: {}", text);
        let key = hex::encode(
            sha2::Sha256::new()
                .chain(self.identifier.as_bytes())
                .chain(text.as_bytes())
                .finalize(),
        );

        if let Some(data) = self.cache.get(&key).await {
            tracing::debug!("cache hit for {} with key {}", &text, &key);
            return Ok(data);
        }

        tracing::debug!(
            "cache miss for {} with key {}, delegate request",
            &text,
            &key
        );
        let data = self.inner.generate(text).await?;

        self.cache.insert(key, data.clone()).await;

        Ok(data)
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
        assert_eq!(result.to_vec(), b"hello");
        assert_eq!(
            mock.call_count(),
            1,
            "First call should hit the inner voice"
        );

        let result = cached_voice.generate(text).await.unwrap();
        assert_eq!(result.to_vec(), b"hello");
        assert_eq!(mock.call_count(), 1, "Second call should hit the cache");

        // different text
        let _ = cached_voice.generate("world").await;
        assert_eq!(mock.call_count(), 2, "New text should hit the inner voice");
    }
}
