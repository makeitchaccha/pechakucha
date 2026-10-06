use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoicevoxVoiceConfig {
    pub speaker_id: i32,
    pub speed_scale: Option<f64>,
    pub pitch_scale: Option<f64>,
    pub intonation_scale: Option<f64>,
    pub volume_scale: Option<f64>,
    pub pre_phoneme_length: Option<f64>,
    pub post_phoneme_length: Option<f64>,
}

impl VoicevoxVoiceConfig {
    pub fn generate_default_detail(&self, key: &str) -> crate::tts::VoiceDetail {
        crate::tts::VoiceDetail {
            name: key.to_string(),
            provider: "VOICEVOX".to_string(),
            description: None,
        }
    }
}
