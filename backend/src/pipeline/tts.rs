use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;

use crate::config::AppConfig;
use crate::pipeline::storage::StorageClient;

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
    storage: &StorageClient,
) -> Result<()> {
    let cleaned_text = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let cleaned_text = cleaned_text.context("No cleaned_text available for TTS")?;

    let chunks = chunk_text(&cleaned_text, 4000);
    let mut audio_parts: Vec<Bytes> = Vec::new();

    let client = reqwest::Client::new();

    for chunk in &chunks {
        let audio = tts_google(&client, config, chunk).await?;
        audio_parts.push(audio);
    }

    // Concatenate MP3 chunks
    let total_bytes: Vec<u8> = audio_parts.iter().flat_map(|b| b.to_vec()).collect();
    let audio = Bytes::from(total_bytes);

    // Exact MP3 duration
    let duration_secs = mp3_duration::from_read(&mut std::io::Cursor::new(&audio[..]))
        .map(|d| d.as_secs() as i32)
        .unwrap_or_else(|_| {
            // Fallback: estimate from word count
            let word_count = cleaned_text.split_whitespace().count();
            (word_count as f64 / 150.0 * 60.0) as i32
        });

    // Upload to storage
    let audio_url = storage.upload_episode_audio(episode_id, audio).await?;

    sqlx::query("UPDATE episodes SET audio_url = $1, duration_secs = $2 WHERE id = $3")
        .bind(&audio_url)
        .bind(duration_secs)
        .bind(episode_id)
        .execute(pool)
        .await?;

    Ok(())
}

fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for sentence in split_sentences(text) {
        if current.len() + sentence.len() > max_chars && !current.is_empty() {
            chunks.push(current.clone());
            current.clear();
        }
        current.push_str(&sentence);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if (ch == '.' || ch == '!' || ch == '?') && current.len() > 1 {
            sentences.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
}

async fn tts_google(
    client: &reqwest::Client,
    config: &AppConfig,
    text: &str,
) -> Result<Bytes> {
    let url = format!(
        "https://texttospeech.googleapis.com/v1/text:synthesize?key={}",
        config.google_api_key
    );

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "input": { "text": text },
            "voice": {
                "languageCode": "en-US",
                "name": config.google_tts_voice,
            },
            "audioConfig": { "audioEncoding": "MP3" },
        }))
        .send()
        .await?
        .error_for_status()
        .context("Google TTS request failed")?;

    let body: serde_json::Value = resp.json().await?;
    let audio_b64 = body["audioContent"]
        .as_str()
        .context("No audioContent in Google TTS response")?;

    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .context("Failed to decode Google TTS audio")?;

    Ok(Bytes::from(audio_bytes))
}
