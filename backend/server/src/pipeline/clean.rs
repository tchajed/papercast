use anyhow::{Context, Result};

use crate::config::AppConfig;

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let (raw_text, source_type) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT raw_text, source_type FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let raw_text = raw_text.context("No raw_text available for cleaning")?;
    tracing::info!(
        "Clean start: episode={episode_id} source_type={source_type} raw_chars={}",
        raw_text.len()
    );

    let input_doc = tts_lib::Document {
        raw_text: Some(raw_text),
        source_type,
        ..Default::default()
    };

    let provider = config.make_provider();
    let (doc, usage) = tts_lib::clean::clean(&input_doc, &provider)
        .await
        .with_context(|| format!("Clean failed for episode {episode_id}"))?;
    crate::usage::record(pool, Some(episode_id), None, "clean", &usage).await;

    let cleaned_text = doc
        .cleaned_text
        .context("No cleaned_text returned from cleaning")?;
    let word_count = doc.word_count.unwrap_or(0) as i32;

    sqlx::query("UPDATE episodes SET cleaned_text = $1, word_count = $2 WHERE id = $3")
        .bind(&cleaned_text)
        .bind(word_count)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!(
        "Clean done: episode={episode_id} cleaned_chars={} word_count={word_count}",
        cleaned_text.len()
    );
    Ok(())
}
