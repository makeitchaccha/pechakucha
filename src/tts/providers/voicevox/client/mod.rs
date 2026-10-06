mod query;

pub(in crate::tts::providers::voicevox) use query::LazyAudioQuery;

/// minimum client for Voicevox
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base_url: reqwest::Url,
    request_timeout: std::time::Duration,
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        base_url: reqwest::Url,
        request_timeout: std::time::Duration,
    ) -> Client {
        Client {
            http,
            base_url,
            request_timeout,
        }
    }

    pub(super) async fn audio_query(
        &self,
        text: &str,
        speaker: i32,
    ) -> anyhow::Result<LazyAudioQuery> {
        let url = self.base_url.join("/audio_query")?;
        let request = self
            .http
            .post(url)
            .query(&[("text", text), ("speaker", &speaker.to_string())])
            .header(reqwest::header::ACCEPT, "application/json");
        let audio_query = tokio::time::timeout(self.request_timeout, async {
            request
                .send()
                .await?
                .error_for_status()?
                .json::<LazyAudioQuery>()
                .await
                .map_err(anyhow::Error::from)
        })
        .await??;
        Ok(audio_query)
    }

    pub(super) async fn streaming_synthesis(
        &self,
        speaker: i32,
        segment_length: f64,
        audio_query: LazyAudioQuery,
    ) -> anyhow::Result<StreamingResponse> {
        let url = self.base_url.join("/streaming_synthesis")?;
        let request = self
            .http
            .post(url)
            .query(&[
                ("speaker", speaker.to_string()),
                ("segment_length", segment_length.to_string()),
            ])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "audio/wav")
            .json(&audio_query);
        let response = tokio::time::timeout(self.request_timeout, request.send())
            .await??
            .error_for_status()?;

        Ok(StreamingResponse {
            content_length: response.content_length(),
            idle_timeout: self.request_timeout,
            body: Box::pin(response.bytes_stream()),
        })
    }

    pub(super) async fn synthesis(
        &self,
        speaker: i32,
        audio_query: LazyAudioQuery,
    ) -> anyhow::Result<Vec<u8>> {
        let url = self.base_url.join("/synthesis")?;
        let request = self
            .http
            .post(url)
            .query(&[("speaker", speaker.to_string())])
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "audio/wav")
            .json(&audio_query);
        let response = tokio::time::timeout(self.request_timeout, request.send())
            .await??
            .error_for_status()?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }
}

type ResponseBody = std::pin::Pin<
    Box<dyn futures_util::Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send>,
>;

pub(super) struct StreamingResponse {
    body: ResponseBody,
    content_length: Option<u64>,
    idle_timeout: std::time::Duration,
}

impl StreamingResponse {
    pub(super) fn content_length(&self) -> Option<u64> {
        self.content_length
    }

    pub(super) async fn next_chunk(&mut self) -> anyhow::Result<Option<bytes::Bytes>> {
        use futures_util::StreamExt;

        match tokio::time::timeout(self.idle_timeout, self.body.next()).await? {
            None => Ok(None),
            Some(Ok(bytes)) => Ok(Some(bytes)),
            Some(Err(error)) => Err(error.into()),
        }
    }
}
