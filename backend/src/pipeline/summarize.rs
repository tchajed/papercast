use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

const SUMMARIZE_SYSTEM_PROMPT: &str = r#"You are preparing a concise podcast-style summary of a text.
Condense the content into a clear, engaging summary suitable for listening.

Rules:
- Capture the key ideas, findings, and arguments.
- Aim for roughly 20-30% of the original length.
- Use natural, spoken-style language — this will be read aloud by TTS.
- Maintain the logical flow: introduce the topic, cover main points, conclude.
- Do not add your own opinions or commentary.
- Do not use bullet points or numbered lists — write in flowing paragraphs.
- Output only the summary text, nothing else."#;

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    temperature: f32,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
}

#[derive(Deserialize)]
struct ClaudeContent {
    text: String,
}

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let cleaned_text = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let cleaned_text = cleaned_text.context("No cleaned_text available for summarization")?;

    let client = reqwest::Client::new();
    let request = ClaudeRequest {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 8192,
        temperature: 0.0,
        system: SUMMARIZE_SYSTEM_PROMPT.to_string(),
        messages: vec![ClaudeMessage {
            role: "user".to_string(),
            content: cleaned_text,
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &config.anthropic_api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?
        .error_for_status()
        .context("Claude API request failed during summarization")?;

    let claude_resp: ClaudeResponse = resp.json().await?;
    let transcript = claude_resp
        .content
        .first()
        .map(|c| c.text.clone())
        .context("Empty response from Claude during summarization")?;

    let word_count = transcript.split_whitespace().count() as i32;
    tracing::info!(
        "Summarization complete for episode {episode_id}: {word_count} words"
    );

    sqlx::query("UPDATE episodes SET transcript = $1, word_count = $2 WHERE id = $3")
        .bind(&transcript)
        .bind(word_count)
        .bind(episode_id)
        .execute(pool)
        .await?;

    Ok(())
}
