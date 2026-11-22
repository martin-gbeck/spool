use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Error, bail};
use base64::Engine as _;

use crate::config::Config;

/// Per-request overrides, resolved against the config once per speak call
/// rather than once per chunk.
#[derive(Debug, Clone)]
pub struct SpeechRequest {
    pub voice: String,
    pub instructions: String,
}

pub async fn get_audio_bytes(
    text: String,
    config: Arc<Config>,
    request: SpeechRequest,
) -> Result<Vec<u8>, Error> {
    // Without a timeout a request that never answers would hold its chunk,
    // and everything queued behind it, forever.
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(150))
        .build()?;

    if config.engine == "openai" {
        openai(&client, &text, &config, request).await
    } else {
        speechify(&client, &text, &config, request).await
    }
}

async fn openai(
    client: &reqwest::Client,
    text: &str,
    config: &Config,
    request: SpeechRequest,
) -> Result<Vec<u8>, Error> {
    if config.resolved_api_key.is_empty() {
        bail!("no OpenAI api key");
    }
    let mut body = serde_json::json!({
        "model": config.model,
        "voice": request.voice,
        "input": text,
    });

    // "instructions" is only valid for the gpt-4o-mini-tts model
    if config.model == "gpt-4o-mini-tts" && !request.instructions.is_empty() {
        body["instructions"] = serde_json::Value::String(request.instructions);
    }

    let response = client
        .post("https://api.openai.com/v1/audio/speech")
        .bearer_auth(&config.resolved_api_key)
        .json(&body)
        .send()
        .await?;

    let response = response.error_for_status()?;

    let audio_bytes = response.bytes().await?;

    Ok(audio_bytes.to_vec())
}

/// Speechify answers with JSON whose `audio_data` is the mp3 in base64. No tone
/// prompt exists there, so `instructions` is ignored.
async fn speechify(
    client: &reqwest::Client,
    text: &str,
    config: &Config,
    request: SpeechRequest,
) -> Result<Vec<u8>, Error> {
    if config.resolved_speechify_key.is_empty() {
        bail!("no Speechify api key");
    }
    let body = serde_json::json!({
        "input": text,
        "voice_id": request.voice,
        "model": config.speechify_model,
        "audio_format": "mp3",
    });

    let response = client
        .post("https://api.sws.speechify.com/v1/audio/speech")
        .bearer_auth(&config.resolved_speechify_key)
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    let reply: serde_json::Value = response.json().await?;
    decode_audio(&reply)
}

fn decode_audio(reply: &serde_json::Value) -> Result<Vec<u8>, Error> {
    let encoded = reply
        .get("audio_data")
        .and_then(|v| v.as_str())
        .context("Speechify reply has no audio_data")?;
    Ok(base64::engine::general_purpose::STANDARD.decode(encoded)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_data_is_decoded_from_base64() {
        let reply = serde_json::json!({ "audio_data": "SUQz" });
        assert_eq!(decode_audio(&reply).unwrap(), b"ID3");
    }

    #[test]
    fn a_reply_without_audio_data_is_an_error() {
        assert!(decode_audio(&serde_json::json!({ "error": "x" })).is_err());
    }
}
