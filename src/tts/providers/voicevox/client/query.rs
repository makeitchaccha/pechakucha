use crate::tts::VoiceError;
use crate::tts::providers::voicevox::config::VoicevoxVoiceConfig;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(in crate::tts::providers::voicevox) struct LazyAudioQuery {
    accent_phrases: Box<serde_json::value::RawValue>,
    #[serde(rename = "speedScale")]
    speed_scale: f64,
    #[serde(rename = "pitchScale")]
    pitch_scale: f64,
    #[serde(rename = "intonationScale")]
    intonation_scale: f64,
    #[serde(rename = "volumeScale")]
    volume_scale: f64,
    #[serde(rename = "prePhonemeLength")]
    pre_phoneme_length: f64,
    #[serde(rename = "postPhonemeLength")]
    post_phoneme_length: f64,
    #[serde(rename = "pauseLength")]
    pause_length: Option<f64>,
    #[serde(rename = "pauseLengthScale")]
    pause_length_scale: Option<f64>,
    #[serde(rename = "outputSamplingRate")]
    output_sampling_rate: u32,
    #[serde(rename = "outputStereo")]
    output_stereo: bool,
    kana: Option<String>,
}

impl LazyAudioQuery {
    pub(in crate::tts::providers::voicevox) fn apply_config(
        &mut self,
        config: &VoicevoxVoiceConfig,
    ) {
        if let Some(s) = config.speed_scale {
            self.speed_scale = s;
        }
        if let Some(p) = config.pitch_scale {
            self.pitch_scale = p;
        }
        if let Some(i) = config.intonation_scale {
            self.intonation_scale = i;
        }
        if let Some(v) = config.volume_scale {
            self.volume_scale = v;
        }
        if let Some(l) = config.pre_phoneme_length {
            self.pre_phoneme_length = l;
        }
        if let Some(l) = config.post_phoneme_length {
            self.post_phoneme_length = l;
        }
    }
}

#[derive(Deserialize)]
struct RawAccentPhrase {
    moras: Vec<RawMora>,
    pause_mora: Option<RawMora>,
}

#[derive(Deserialize)]
struct RawMora {
    consonant_length: Option<f64>,
    vowel_length: f64,
}

impl LazyAudioQuery {
    pub(in crate::tts::providers::voicevox) fn estimated_audio_duration(
        &self,
    ) -> Result<std::time::Duration, VoiceError> {
        let phrases: Vec<RawAccentPhrase> = serde_json::from_str(self.accent_phrases.get())
            .map_err(|error| {
                VoiceError::Api(anyhow::anyhow!(
                    "Failed to parse VOICEVOX accent phrases for audio duration: {error}"
                ))
            })?;
        let phrase_seconds: f64 = phrases
            .iter()
            .map(|phrase| {
                phrase
                    .moras
                    .iter()
                    .map(|mora| mora.consonant_length.unwrap_or(0.0) + mora.vowel_length)
                    .sum::<f64>()
                    + phrase.pause_mora.as_ref().map_or(0.0, |mora| {
                        mora.consonant_length.unwrap_or(0.0) + mora.vowel_length
                    })
            })
            .sum();
        let pause_seconds =
            self.pause_length.unwrap_or(0.0) * self.pause_length_scale.unwrap_or(1.0);
        let total_seconds = phrase_seconds / self.speed_scale
            + pause_seconds
            + self.pre_phoneme_length
            + self.post_phoneme_length;
        if !total_seconds.is_finite() || total_seconds <= 0.0 {
            return Err(VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX returned an invalid estimated audio duration: {total_seconds}"
            )));
        }
        let duration = std::time::Duration::try_from_secs_f64(total_seconds).map_err(|error| {
            VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX estimated audio duration is out of range: {error}"
            ))
        })?;
        if duration.is_zero() {
            return Err(VoiceError::Api(anyhow::anyhow!(
                "VOICEVOX estimated audio duration rounded to zero"
            )));
        }
        Ok(duration)
    }
}
