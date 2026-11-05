//! The voice part of the moby-api client: consent, speech to text, text to speech. It goes through
//! the same `Api` as everything else, so the token stays in Rust and a 401 refreshes it the same
//! way. Every call has a time limit, and dropping its future drops the request.
//!
//! The contract is `POST /v1/voice/stt` (a WAV body, optional `language` and `script` query) and
//! `POST /v1/voice/tts` (JSON with 1 to 500 characters), both answering `403 voice_consent_required`
//! unless the account has accepted the current `cloud_voice` version. Audio and text travel in the
//! request and are never written anywhere here.

use std::time::Duration;

use futures_util::StreamExt;
use moby_protocol::{Language, Script};
use reqwest::Method;
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::api::{Api, Failure, failure_from};

use super::wav::MAX_WAV_BYTES;

/// The TTS text limit of the API, in characters.
pub const MAX_TTS_CHARS: usize = 500;
/// The most text a transcript can carry into a turn. The agent takes 8 KiB per message.
pub const MAX_TRANSCRIPT_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Transcript {
    pub text: String,
    pub language: Language,
    pub script: Script,
    pub confidence: f32,
}

#[derive(Deserialize)]
struct RawTranscript {
    text: String,
    language: String,
    script: String,
    confidence: f32,
}

fn parse_language(value: &str) -> Option<Language> {
    match value {
        "uz" => Some(Language::Uz),
        "ru" => Some(Language::Ru),
        "en" => Some(Language::En),
        _ => None,
    }
}

fn parse_script(value: &str) -> Option<Script> {
    match value {
        "latn" => Some(Script::Latn),
        "cyrl" => Some(Script::Cyrl),
        _ => None,
    }
}

pub fn language_str(language: Language) -> &'static str {
    match language {
        Language::Uz => "uz",
        Language::Ru => "ru",
        Language::En => "en",
    }
}

pub fn script_str(script: Script) -> &'static str {
    match script {
        Script::Latn => "latn",
        Script::Cyrl => "cyrl",
    }
}

/// Cuts at a character boundary.
pub fn clip_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// What to speak of an answer: at most 500 characters, cut after a sentence end if there is one.
pub fn speakable(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() <= MAX_TTS_CHARS {
        return text.to_owned();
    }
    let head: String = text.chars().take(MAX_TTS_CHARS).collect();
    match head.rfind(['.', '!', '?']) {
        Some(end) if end > 0 => head[..=end].to_owned(),
        _ => head,
    }
}

/// What the person is told for a failed voice call. The server's own words are not used: they
/// could carry provider text.
fn voice_failure(code: &str, status: u16) -> Failure {
    let (code, message) = match (code, status) {
        ("voice_consent_required", _) | (_, 403) => (
            "voice_consent_required",
            "Voice is off for your account. Turn it on in Settings to talk to Moby.",
        ),
        (_, 413) => (
            "voice_too_long",
            "That was too long for the server to take.",
        ),
        (_, 429) => (
            "voice_quota",
            "Today's voice allowance is used up. It resets at midnight UTC.",
        ),
        (_, 502 | 504) => (
            "voice_provider",
            "The speech service did not answer. Try again in a moment.",
        ),
        (_, 503) => (
            "voice_unavailable",
            "Voice is not available on the server right now.",
        ),
        _ => ("voice_failed", "The voice request failed. Try again."),
    };
    Failure::new(code, message)
}

async fn voice_error(response: reqwest::Response) -> Failure {
    let status = response.status().as_u16();
    let failure = failure_from(response).await;
    voice_failure(failure.code_str(), status)
}

/// Sends a whole clip to speech to text.
pub async fn transcribe(
    api: &Api,
    wave: Vec<u8>,
    timeout: Duration,
) -> Result<Transcript, Failure> {
    let response = api
        .send_with(Method::POST, "/v1/voice/stt", Some(timeout), |request| {
            request.header(CONTENT_TYPE, "audio/wav").body(wave.clone())
        })
        .await?;
    if !response.status().is_success() {
        return Err(voice_error(response).await);
    }
    let raw: RawTranscript = response.json().await.map_err(|error| {
        if error.is_timeout() {
            Failure::new(
                "timeout",
                "Moby's server took too long to answer. Try again.",
            )
        } else {
            Failure::new("voice_failed", "The voice request failed. Try again.")
        }
    })?;
    let bad = || Failure::new("voice_failed", "The voice request failed. Try again.");
    Ok(Transcript {
        text: clip_bytes(raw.text.trim(), MAX_TRANSCRIPT_BYTES).to_owned(),
        language: parse_language(&raw.language).ok_or_else(bad)?,
        script: parse_script(&raw.script).ok_or_else(bad)?,
        confidence: if raw.confidence.is_finite() {
            raw.confidence.clamp(0.0, 1.0)
        } else {
            0.0
        },
    })
}

/// Turns text into a WAV. The answer is read with a size limit.
pub async fn synthesize(
    api: &Api,
    text: &str,
    language: Language,
    script: Script,
    timeout: Duration,
) -> Result<Vec<u8>, Failure> {
    let body: Value = json!({
        "text": text,
        "language": language_str(language),
        "script": script_str(script),
    });
    let response = api
        .send_with(Method::POST, "/v1/voice/tts", Some(timeout), |request| {
            request.json(&body)
        })
        .await?;
    if !response.status().is_success() {
        return Err(voice_error(response).await);
    }
    let mut wave = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            if error.is_timeout() {
                Failure::new(
                    "timeout",
                    "Moby's server took too long to answer. Try again.",
                )
            } else {
                Failure::new(
                    "offline",
                    "Can't connect to Moby. Check your connection and try again.",
                )
            }
        })?;
        if wave.len() + chunk.len() > MAX_WAV_BYTES {
            return Err(Failure::new(
                "voice_failed",
                "The voice request failed. Try again.",
            ));
        }
        wave.extend_from_slice(&chunk);
    }
    Ok(wave)
}

/// The `cloud_voice` version the server wants accepted now, and the consent text if the server
/// sends one (`GET /v1/consents` returns versions only today, so the text is usually absent).
pub struct ConsentOffer {
    pub version: String,
    pub text: Option<String>,
}

pub async fn current_consent_offer(api: &Api) -> Result<ConsentOffer, Failure> {
    let (version, text) = fetch_consent(api).await?;
    Ok(ConsentOffer { version, text })
}

pub async fn current_consent_version(api: &Api) -> Result<String, Failure> {
    Ok(fetch_consent(api).await?.0)
}

async fn fetch_consent(api: &Api) -> Result<(String, Option<String>), Failure> {
    let response = api
        .send_with(Method::GET, "/v1/consents", None, |r| r)
        .await?;
    if !response.status().is_success() {
        return Err(failure_from(response).await);
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| Failure::new("unexpected", "Something went wrong. Try again."))?;
    let version = body["current_versions"]["cloud_voice"]
        .as_str()
        .filter(|version| !version.is_empty() && version.len() <= 64)
        .map(str::to_owned)
        .ok_or_else(|| Failure::new("unexpected", "Something went wrong. Try again."))?;
    let text = body["texts"]["cloud_voice"]
        .as_str()
        .map(|text| clip_bytes(text, 600).to_owned())
        .filter(|text| !text.trim().is_empty());
    Ok((version, text))
}

/// Accepts or withdraws the `cloud_voice` consent on the server.
pub async fn set_consent(api: &Api, version: &str, accepted: bool) -> Result<(), Failure> {
    let body = json!({"type": "cloud_voice", "version": version, "accepted": accepted});
    let response = api
        .send_with(Method::POST, "/v1/consents", None, |request| {
            request.json(&body)
        })
        .await?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(failure_from(response).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_is_cut_to_the_api_limit_after_a_sentence_when_possible() {
        assert_eq!(speakable("  Hello. "), "Hello.");
        let long = format!(
            "{} Next one starts here and runs on {}",
            "Short one.",
            "x".repeat(600)
        );
        let said = speakable(&long);
        assert_eq!(said, "Short one.");
        let no_stop = "y".repeat(900);
        assert_eq!(speakable(&no_stop).chars().count(), MAX_TTS_CHARS);
        let wide = "ў".repeat(900);
        assert_eq!(speakable(&wide).chars().count(), MAX_TTS_CHARS);
    }

    #[test]
    fn clipping_never_splits_a_character() {
        let text = "аб".repeat(10);
        for max in 0..text.len() {
            assert!(text.is_char_boundary(clip_bytes(&text, max).len()));
        }
    }

    #[test]
    fn failures_are_written_by_the_app() {
        assert_eq!(
            voice_failure("voice_consent_required", 403).code_str(),
            "voice_consent_required"
        );
        assert_eq!(voice_failure("whatever", 429).code_str(), "voice_quota");
        assert_eq!(
            voice_failure("provider says: secret", 502).code_str(),
            "voice_provider"
        );
        assert!(
            !voice_failure("provider says: secret", 502)
                .message_str()
                .contains("secret")
        );
    }
}
