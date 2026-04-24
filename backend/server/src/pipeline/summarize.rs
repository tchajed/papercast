use anyhow::{Context, Result};

use crate::config::AppConfig;

pub async fn run(episode_id: &str, pool: &sqlx::SqlitePool, config: &AppConfig) -> Result<()> {
    let (cleaned_text, focus) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT cleaned_text, summarize_focus FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let cleaned_text = cleaned_text.context("No cleaned_text available for summarization")?;
    tracing::info!(
        "Summarize start: episode={episode_id} cleaned_chars={}",
        cleaned_text.len()
    );

    let input_doc = tts_lib::Document {
        cleaned_text: Some(cleaned_text),
        ..Default::default()
    };

    let provider = config.make_provider();
    let (doc, usage) = tts_lib::summarize::summarize(&input_doc, &provider, focus.as_deref())
        .await
        .with_context(|| format!("Summarize failed for episode {episode_id}"))?;
    crate::usage::record(pool, Some(episode_id), None, "summarize", &usage).await;

    let transcript = doc
        .transcript
        .context("No transcript returned from summarization")?;
    let word_count = doc.word_count.unwrap_or(0) as i32;

    sqlx::query("UPDATE episodes SET transcript = $1, word_count = $2 WHERE id = $3")
        .bind(&transcript)
        .bind(word_count)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!(
        "Summarize done: episode={episode_id} transcript_chars={} word_count={word_count}",
        transcript.len()
    );
    Ok(())
}
