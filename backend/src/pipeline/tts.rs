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
    // Use transcript (from summarization) if available, otherwise cleaned_text
    let (transcript, cleaned_text) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT transcript, cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let tts_text = transcript
        .or(cleaned_text)
        .context("No text available for TTS")?;

    let word_count = tts_text.split_whitespace().count();
    let chunks = chunk_text(&tts_text, 4000);
    let total_chunks = chunks.len();
    let mut audio_parts: Vec<Bytes> = Vec::new();

    tracing::info!(
        "TTS starting for episode {episode_id}: {word_count} words, {total_chunks} chunks (~{:.0}s estimated)",
        word_count as f64 * 0.13
    );

    // Record total chunks for progress tracking
    sqlx::query("UPDATE episodes SET tts_chunks_done = 0, tts_chunks_total = $1 WHERE id = $2")
        .bind(total_chunks as i32)
        .bind(episode_id)
        .execute(pool)
        .await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    for (i, chunk) in chunks.iter().enumerate() {
        let chunk_words = chunk.split_whitespace().count();
        tracing::info!(
            "TTS chunk {}/{} for episode {episode_id} ({chunk_words} words)",
            i + 1,
            total_chunks,
        );
        let audio = tts_google(&client, config, chunk).await?;
        audio_parts.push(audio);

        // Update progress
        sqlx::query("UPDATE episodes SET tts_chunks_done = $1 WHERE id = $2")
            .bind((i + 1) as i32)
            .bind(episode_id)
            .execute(pool)
            .await?;
    }

    // Concatenate MP3 chunks
    let total_bytes: Vec<u8> = audio_parts.iter().flat_map(|b| b.to_vec()).collect();
    let audio = Bytes::from(total_bytes);

    // Exact MP3 duration
    let duration_secs = mp3_duration::from_read(&mut std::io::Cursor::new(&audio[..]))
        .map(|d| d.as_secs() as i32)
        .unwrap_or_else(|_| {
            // Fallback: estimate from word count
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_sentences_basic() {
        let result = split_sentences("Hello world. How are you? Fine!");
        assert_eq!(result, vec!["Hello world.", " How are you?", " Fine!"]);
    }

    #[test]
    fn test_split_sentences_no_punctuation() {
        let result = split_sentences("No ending punctuation");
        assert_eq!(result, vec!["No ending punctuation"]);
    }

    #[test]
    fn test_split_sentences_empty() {
        let result = split_sentences("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_chunk_text_single_chunk() {
        let text = "Short text.";
        let chunks = chunk_text(text, 100);
        assert_eq!(chunks, vec!["Short text."]);
    }

    #[test]
    fn test_chunk_text_splits_on_sentence_boundary() {
        let text = "First sentence. Second sentence. Third sentence.";
        let chunks = chunk_text(text, 20);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0], "First sentence.");
        assert_eq!(chunks[1], " Second sentence.");
        assert_eq!(chunks[2], " Third sentence.");
    }

    #[test]
    fn test_chunk_text_empty() {
        let chunks = chunk_text("", 100);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn test_chunk_text_long_sentence_not_split() {
        // A single sentence longer than max_chars stays in one chunk
        let text = "This is a very long sentence that exceeds the max.";
        let chunks = chunk_text(text, 10);
        assert_eq!(chunks, vec!["This is a very long sentence that exceeds the max."]);
    }

    #[test]
    fn test_chunk_text_respects_max_chars() {
        let sentences: Vec<String> = (0..10).map(|i| format!("Sentence {i}.")).collect();
        let text = sentences.join(" ");
        let chunks = chunk_text(&text, 50);
        for chunk in &chunks {
            // Each chunk should be under max_chars (unless a single sentence exceeds it)
            assert!(chunk.len() <= 50 || !chunk.contains(". "));
        }
        // Reassembled text should match original
        let reassembled: String = chunks.join("");
        assert_eq!(reassembled, text);
    }
}

async fn tts_google(
    client: &reqwest::Client,
    config: &AppConfig,
    text: &str,
) -> Result<Bytes> {
    let url = format!(
        "https://texttospeech.googleapis.com/v1/text:synthesize?key={}",
        config.google_tts_api_key
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
