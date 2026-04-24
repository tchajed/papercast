use anyhow::{Context, Result};

use crate::config::AppConfig;

pub async fn run(episode_id: &str, pool: &sqlx::SqlitePool, config: &AppConfig) -> Result<()> {
    let (transcript, cleaned_text) = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT transcript, cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    tracing::info!("Describe start: episode={episode_id}");
    let doc = tts_lib::Document {
        transcript,
        cleaned_text,
        ..Default::default()
    };

    let provider = config.make_provider();
    let (description, usage) = tts_lib::describe::describe(&doc, &provider)
        .await
        .with_context(|| format!("Describe failed for episode {episode_id}"))?;
    crate::usage::record(pool, Some(episode_id), None, "describe", &usage).await;

    sqlx::query("UPDATE episodes SET description = $1 WHERE id = $2")
        .bind(&description)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!(
        "Describe done: episode={episode_id} description_chars={}",
        description.len()
    );
    Ok(())
}
