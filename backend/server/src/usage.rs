use sqlx::SqlitePool;
use tts_lib::Usage;

/// Record an AI usage event. Failures are logged but not propagated; usage
/// accounting should never break the pipeline.
pub async fn record(
    pool: &SqlitePool,
    episode_id: Option<&str>,
    feed_id: Option<&str>,
    stage: &str,
    usage: &Usage,
) {
    let res = sqlx::query(
        "INSERT INTO ai_usage (episode_id, feed_id, stage, provider, model, input_tokens, output_tokens)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(episode_id)
    .bind(feed_id)
    .bind(stage)
    .bind(&usage.provider)
    .bind(&usage.model)
    .bind(usage.input_tokens as i64)
    .bind(usage.output_tokens as i64)
    .execute(pool)
    .await;
    if let Err(e) = res {
        tracing::warn!("Failed to record usage ({stage}): {e}");
    }
}

pub async fn record_many(
    pool: &SqlitePool,
    episode_id: Option<&str>,
    feed_id: Option<&str>,
    stage: &str,
    usages: &[Usage],
) {
    for u in usages {
        record(pool, episode_id, feed_id, stage, u).await;
    }
}
