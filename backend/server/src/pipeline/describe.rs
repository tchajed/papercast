use anyhow::{Context, Result};

use crate::config::AppConfig;

const SYSTEM_PROMPT: &str = r#"You are writing a short description for a podcast episode, to appear in a podcast feed.

Rules:
- 1-3 sentences, under 400 characters total.
- Summarize what the episode is about so a listener can decide whether to play it.
- Plain prose, no headings or bullets. No quoting, no leading label.
- Do not start with phrases like "This episode" or "In this paper" — just describe the content."#;

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let (transcript, cleaned_text) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT transcript, cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let text = transcript
        .or(cleaned_text)
        .context("No transcript or cleaned_text available for description")?;

    // Cap input to keep prompt small; descriptions summarize, so the first
    // few thousand chars are plenty.
    let snippet: String = text.chars().take(8000).collect();

    let provider = config.make_provider();
    let client = reqwest::Client::new();
    let description = provider
        .chat(
            &client,
            "claude-sonnet-4-6",
            Some(SYSTEM_PROMPT),
            &snippet,
            400,
        )
        .await?;
    let description = description.trim().to_string();

    sqlx::query("UPDATE episodes SET description = $1 WHERE id = $2")
        .bind(&description)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!("Generated description for episode {episode_id}");
    Ok(())
}
