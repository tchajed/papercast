use anyhow::{Context, Result};

use crate::config::AppConfig;
use crate::pipeline::storage::StorageClient;

/// Generate a cover image for an episode.
/// This runs after the episode is already in 'done' state.
/// Failures are logged but don't affect episode availability.
pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
    storage: &StorageClient,
) -> Result<()> {
    if !config.generate_images {
        tracing::debug!("Image generation disabled, skipping");
        return Ok(());
    }

    let cleaned_text = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?
    .context("No cleaned_text for image generation")?;

    let provider = config.make_provider();
    let summary = tts_lib::image::visual_summary(&cleaned_text, &provider).await?;
    let image = tts_lib::image::generate_image(&config.google_studio_api_key, &summary).await?;

    let image_url = storage
        .upload_episode_image(episode_id, image.bytes, &image.mime_type)
        .await?;

    sqlx::query("UPDATE episodes SET image_url = $1 WHERE id = $2")
        .bind(&image_url)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!("Generated cover image for episode {episode_id}");
    Ok(())
}
