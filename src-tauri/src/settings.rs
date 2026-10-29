use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub groq_key: String,
    /// Whisper language code ("ru", "en", ...); empty = auto.
    pub stt_language: String,
    /// Language of generated answers, free text ("русский", "English").
    pub answer_language: String,
    pub model: String,
    pub resume: String,
    pub vacancy: String,
    pub style: String,
    /// RMS level (0..1) above which audio counts as speech.
    pub vad_threshold: f32,
    /// Silence that ends an utterance.
    pub pause_ms: u32,
    pub min_utterance_ms: u32,
    pub max_utterance_s: u32,
    /// Background opacity of the overlay, 0.2..1.
    pub opacity: f32,
    pub font_size: u32,
    pub use_mic: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            groq_key: String::new(),
            stt_language: "ru".into(),
            answer_language: "русский".into(),
            model: "openai/gpt-oss-120b".into(),
            resume: String::new(),
            vacancy: String::new(),
            style: "уверенно, спокойно, по делу, разговорный язык".into(),
            vad_threshold: 0.015,
            pause_ms: 1200,
            min_utterance_ms: 700,
            max_utterance_s: 25,
            opacity: 0.7,
            font_size: 17,
            use_mic: false,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())
    }

    pub fn sanitize(&mut self) {
        self.vad_threshold = self.vad_threshold.clamp(0.0005, 0.5);
        self.pause_ms = self.pause_ms.clamp(300, 5000);
        self.min_utterance_ms = self.min_utterance_ms.clamp(200, 5000);
        self.max_utterance_s = self.max_utterance_s.clamp(5, 60);
        self.opacity = self.opacity.clamp(0.2, 1.0);
        self.font_size = self.font_size.clamp(10, 40);
        self.groq_key = self.groq_key.trim().to_string();
        self.model = self.model.trim().to_string();
        if self.model.is_empty() {
            self.model = Settings::default().model;
        }
        self.stt_language = self.stt_language.trim().to_lowercase();
    }

    /// Copy safe to hand to the frontend: the API key never leaves Rust.
    pub fn public(&self) -> serde_json::Value {
        let mut s = self.clone();
        s.groq_key.clear();
        let mut v = serde_json::to_value(s).unwrap_or_default();
        v["has_key"] = serde_json::Value::Bool(!self.groq_key.is_empty());
        v
    }
}
