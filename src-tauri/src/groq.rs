//! Groq API (OpenAI-compatible): Whisper transcription and streaming chat.
//! The API key is only used in request headers and is never logged.

use futures_util::StreamExt;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const BASE: &str = "https://api.groq.com/openai/v1";
const STT_MODEL: &str = "whisper-large-v3-turbo";

pub enum ApiError {
    Auth,
    RateLimit(Option<String>),
    Network,
    Http(u16, String),
    Other(String),
}

impl ApiError {
    pub fn message(&self) -> String {
        match self {
            ApiError::Auth => "Groq: неверный ключ API — проверьте настройки".into(),
            ApiError::RateLimit(Some(s)) => format!("Groq: лимит запросов, повтор через {s} с"),
            ApiError::RateLimit(None) => "Groq: превышен лимит запросов, подождите".into(),
            ApiError::Network => "Нет связи с Groq — проверьте интернет".into(),
            ApiError::Http(c, m) => format!("Groq: ошибка {c}: {m}"),
            ApiError::Other(m) => format!("Groq: {m}"),
        }
    }
}

fn net(_: reqwest::Error) -> ApiError {
    ApiError::Network
}

async fn check(resp: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let retry = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = resp.text().await.unwrap_or_default();
    let msg = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or(body);
    let msg: String = msg.chars().take(160).collect();
    Err(match status.as_u16() {
        401 => ApiError::Auth,
        429 => ApiError::RateLimit(retry),
        c => ApiError::Http(c, msg),
    })
}

pub fn client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .build()
        .expect("http client")
}

/// 16 kHz mono 16-bit PCM WAV, built in memory.
pub fn wav_bytes(pcm: &[i16]) -> Vec<u8> {
    let rate = crate::audio::RATE;
    let data_len = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

pub async fn transcribe(c: &Client, key: &str, wav: Vec<u8>, lang: &str) -> Result<String, ApiError> {
    let part = Part::bytes(wav)
        .file_name("speech.wav")
        .mime_str("audio/wav")
        .map_err(|e| ApiError::Other(e.to_string()))?;
    let mut form = Form::new()
        .part("file", part)
        .text("model", STT_MODEL)
        .text("response_format", "verbose_json")
        .text("temperature", "0");
    if !lang.is_empty() {
        form = form.text("language", lang.to_string());
    }
    let resp = c
        .post(format!("{BASE}/audio/transcriptions"))
        .bearer_auth(key)
        .multipart(form)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(net)?;
    let v: Value = check(resp).await?.json().await.map_err(net)?;
    // Drop segments Whisper itself considers non-speech (music, noise).
    if let Some(segs) = v["segments"].as_array() {
        if !segs.is_empty() {
            let text: String = segs
                .iter()
                .filter(|s| s["no_speech_prob"].as_f64().unwrap_or(0.0) < 0.7)
                .filter_map(|s| s["text"].as_str())
                .collect();
            return Ok(text.trim().to_string());
        }
    }
    Ok(v["text"].as_str().unwrap_or("").trim().to_string())
}

/// Streams chat completion deltas into `on_delta`; returning `false` stops the stream.
pub async fn chat_stream(
    c: &Client,
    key: &str,
    model: &str,
    messages: Vec<Value>,
    mut on_delta: impl FnMut(&str) -> bool,
) -> Result<(), ApiError> {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "temperature": 0.4,
        "max_tokens": 1024,
    });
    // Reasoning models: keep thinking short so the answer starts quickly.
    if model.starts_with("openai/gpt-oss") {
        body["reasoning_effort"] = json!("low");
    } else if model.starts_with("qwen/") {
        body["reasoning_effort"] = json!("none");
    }
    let resp = c
        .post(format!("{BASE}/chat/completions"))
        .bearer_auth(key)
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(net)?;
    let mut stream = check(resp).await?.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.map_err(net)?);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(());
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(m) = v["error"]["message"].as_str() {
                return Err(ApiError::Other(m.to_string()));
            }
            if let Some(t) = v["choices"][0]["delta"]["content"].as_str() {
                if !t.is_empty() && !on_delta(t) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::wav_bytes;

    #[test]
    fn wav_header_describes_mono_16khz_pcm() {
        let wav = wav_bytes(&[0x1234, -2]);

        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(&wav[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes(wav[22..24].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[0x34, 0x12, 0xfe, 0xff]);
    }

    #[test]
    fn empty_audio_still_has_a_valid_wav_header() {
        let wav = wav_bytes(&[]);

        assert_eq!(wav.len(), 44);
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 36);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 0);
    }
}
