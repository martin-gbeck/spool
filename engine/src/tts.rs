use std::sync::Arc;
use std::time::Duration;

use anyhow::Error;

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
