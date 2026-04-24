use anyhow::{Context, Result};

use crate::config::AppConfig;
use crate::pipeline::storage::StorageClient;

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
    storage: &StorageClient,
) -> Result<()> {
    let (transcript, cleaned_text, episode_voice) =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
            "SELECT transcript, cleaned_text, tts_voice FROM episodes WHERE id = $1",
        )
        .bind(episode_id)
        .fetch_one(pool)
        .await?;

    let tts_text = transcript
        .or(cleaned_text)
        .context("No text available for TTS")?;

    let voice = episode_voice.unwrap_or_else(|| config.google_tts_voice.clone());
    let tts_config =
        tts_lib::tts::TtsConfig::new(config.google_tts_api_key.clone()).with_voice(voice.clone());

    // Set up progress tracking
    let pool_clone = pool.clone();
    let ep_id = episode_id.to_string();
    let on_progress: tts_lib::tts::ProgressCallback = std::sync::Arc::new(move |done, total| {
        let pool = pool_clone.clone();
        let ep_id = ep_id.clone();
        tokio::spawn(async move {
            let _ = sqlx::query(
                "UPDATE episodes SET tts_chunks_done = $1, tts_chunks_total = $2 WHERE id = $3",
            )
            .bind(done as i32)
            .bind(total as i32)
            .bind(&ep_id)
            .execute(&pool)
            .await;
        });
    });

    // Per-chunk MP3 cache so a retry (scheduled or manual) reuses chunks already
    // synthesized in an earlier attempt. Cleared on success; stale dirs are
    // garbage-collected by `gc_chunk_dirs` on worker startup.
    let cache_dir = format!("/data/{}_tts_chunks", episode_id);
    let result = tts_lib::tts::synthesize(
        &tts_text,
        &tts_config,
        Some(on_progress),
        Some(cache_dir.clone()),
    )
    .await?;

    // TTS cost is per-character. Record char count in input_tokens for a uniform
    // usage schema; output_tokens stays 0.
    crate::usage::record(
        pool,
        Some(episode_id),
        None,
        "tts",
        &tts_lib::Usage {
            provider: "google_tts".into(),
            model: voice,
            input_tokens: tts_text.chars().count() as u32,
            output_tokens: 0,
        },
    )
    .await;

    let audio_with_chapters =
        tts_lib::tts::embed_chapters(&result.audio, &result.sections, result.duration_secs)?;
    let audio_bytes = audio_with_chapters.len() as i64;
    let audio_url = storage
        .upload_episode_audio(episode_id, audio_with_chapters)
        .await?;

    let sections_json = if result.sections.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&result.sections)?)
    };

    sqlx::query(
        "UPDATE episodes SET audio_url = $1, duration_secs = $2, audio_bytes = $3, sections_json = $4 WHERE id = $5",
    )
        .bind(&audio_url)
        .bind(result.duration_secs as i32)
        .bind(audio_bytes)
        .bind(sections_json.as_deref())
        .bind(episode_id)
        .execute(pool)
        .await?;

    let _ = tokio::fs::remove_dir_all(&cache_dir).await;

    Ok(())
}
