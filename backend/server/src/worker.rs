use anyhow::Result;
use sqlx::{FromRow, SqlitePool};
use std::time::Duration;
use crate::config::AppConfig;
use crate::ids::new_id;
use crate::pipeline::storage::StorageClient;

#[derive(Debug, FromRow)]
struct Job {
    id: String,
    episode_id: String,
    job_type: String,
    attempts: i32,
}

pub async fn run_worker(pool: SqlitePool, config: AppConfig, storage: StorageClient) {
    // GC stale per-chunk PDF extraction caches left behind by permanently
    // failed or abandoned jobs. Chunk dirs are only removed on full-extraction
    // success, so cruft accumulates without this sweep.
    tts_lib::pdf_gemini::gc_chunk_dirs("/data", Duration::from_secs(24 * 60 * 60)).await;

    // Recover jobs stuck in 'running' from a previous VM that died mid-job
    match sqlx::query("UPDATE jobs SET status = 'queued' WHERE status = 'running'")
        .execute(&pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => {
            tracing::warn!("Recovered {} orphaned running jobs to queued", res.rows_affected());
        }
        Ok(_) => {}
        Err(e) => tracing::error!("Failed to recover orphaned jobs: {e}"),
    }

    loop {
        match claim_next_job(&pool).await {
            Ok(Some(job)) => {
                // Run inline — SQLite WAL handles concurrent reads from web server
                execute_job(job, &pool, &config, &storage).await;
            }
            Ok(None) => {
                tokio::time::sleep(Duration::from_secs(config.worker_poll_interval)).await;
            }
            Err(e) => {
                tracing::error!("Worker poll error: {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

async fn claim_next_job(pool: &SqlitePool) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;

    let job = sqlx::query_as::<_, Job>(
        "SELECT id, episode_id, job_type, attempts
         FROM jobs
         WHERE status = 'queued' AND run_after <= datetime('now')
         ORDER BY created_at ASC
         LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;

    if let Some(ref job) = job {
        sqlx::query("UPDATE jobs SET status = 'running' WHERE id = $1")
            .bind(&job.id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;
    Ok(job)
}

async fn execute_job(job: Job, pool: &SqlitePool, config: &AppConfig, storage: &StorageClient) {
    tracing::info!(
        "Executing job {} (type={}, episode={}, attempt={})",
        job.id,
        job.job_type,
        job.episode_id,
        job.attempts + 1
    );

    // Update episode status to reflect current stage (except image/describe — status stays 'done')
    if job.job_type != "image" && job.job_type != "describe" {
        let stage_status = match job.job_type.as_str() {
            "scrape" | "pdf" => "scraping",
            "clean" => "cleaning",
            "summarize" => "summarizing",
            "tts" => "tts",
            _ => "error",
        };

        if let Err(e) = sqlx::query("UPDATE episodes SET status = $1 WHERE id = $2")
            .bind(stage_status)
            .bind(&job.episode_id)
            .execute(pool)
            .await
        {
            tracing::error!("Failed to update episode status: {e}");
            return;
        }
    }

    let result = match job.job_type.as_str() {
        "scrape" => crate::pipeline::scrape::run(&job.episode_id, pool, config).await,
        "pdf" => crate::pipeline::pdf::run(&job.episode_id, pool, config).await,
        "clean" => crate::pipeline::clean::run(&job.episode_id, pool, config).await,
        "summarize" => crate::pipeline::summarize::run(&job.episode_id, pool, config).await,
        "tts" => crate::pipeline::tts::run(&job.episode_id, pool, config, storage).await,
        "image" => crate::pipeline::image::run(&job.episode_id, pool, config, storage).await,
        "describe" => crate::pipeline::describe::run(&job.episode_id, pool, config).await,
        other => Err(anyhow::anyhow!("Unknown job type: {other}")),
    };

    match result {
        Ok(_) => {
            if let Err(e) = complete_job(pool, &job, config).await {
                tracing::error!("Failed to complete job {}: {e}", job.id);
            }
        }
        Err(e) => {
            tracing::error!("Job {} failed: {e:?}", job.id);
            // Image and describe failures are non-fatal
            let non_fatal = matches!(job.job_type.as_str(), "image" | "describe");
            if let Err(e2) =
                fail_job(pool, &job, &e.to_string(), config.max_job_attempts, non_fatal).await
            {
                tracing::error!("Failed to record job failure {}: {e2}", job.id);
            }
        }
    }
}

async fn complete_job(pool: &SqlitePool, job: &Job, config: &AppConfig) -> Result<()> {
    let mut tx = pool.begin().await?;

    sqlx::query("UPDATE jobs SET status = 'done' WHERE id = $1")
        .bind(&job.id)
        .execute(&mut *tx)
        .await?;

    // Stage transitions
    match job.job_type.as_str() {
        "scrape" | "pdf" => {
            let job_id = new_id();
            sqlx::query(
                "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'clean', 'queued')",
            )
            .bind(&job_id)
            .bind(&job.episode_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE episodes SET status = 'cleaning' WHERE id = $1")
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
        }
        "clean" => {
            // Check if this episode needs summarization
            let summarize = sqlx::query_scalar::<_, i32>(
                "SELECT summarize FROM episodes WHERE id = $1",
            )
            .bind(&job.episode_id)
            .fetch_one(&mut *tx)
            .await
            .unwrap_or(0);

            if summarize != 0 {
                let job_id = new_id();
                sqlx::query(
                    "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'summarize', 'queued')",
                )
                .bind(&job_id)
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE episodes SET status = 'summarizing' WHERE id = $1")
                    .bind(&job.episode_id)
                    .execute(&mut *tx)
                    .await?;
            } else {
                let job_id = new_id();
                sqlx::query(
                    "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'tts', 'queued')",
                )
                .bind(&job_id)
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
                sqlx::query("UPDATE episodes SET status = 'tts' WHERE id = $1")
                    .bind(&job.episode_id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        "summarize" => {
            let job_id = new_id();
            sqlx::query(
                "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'tts', 'queued')",
            )
            .bind(&job_id)
            .bind(&job.episode_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query("UPDATE episodes SET status = 'tts' WHERE id = $1")
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
        }
        "tts" => {
            // Episode is done; optionally queue image generation
            sqlx::query(
                "UPDATE episodes SET status = 'done', pub_date = datetime('now') WHERE id = $1",
            )
            .bind(&job.episode_id)
            .execute(&mut *tx)
            .await?;

            if config.generate_images {
                let job_id = new_id();
                sqlx::query(
                    "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'image', 'queued')",
                )
                .bind(&job_id)
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
            }

            let describe_id = new_id();
            sqlx::query(
                "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'describe', 'queued')",
            )
            .bind(&describe_id)
            .bind(&job.episode_id)
            .execute(&mut *tx)
            .await?;
        }
        "describe" => {}
        "image" => {
            // Image done — episode.image_url was already patched by the image stage
            // Status stays 'done', no new job
        }
        _ => {}
    }

    tx.commit().await?;

    tracing::info!(
        "Job {} completed (type={}, episode={})",
        job.id,
        job.job_type,
        job.episode_id,
    );

    Ok(())
}

/// Detects upstream model provider outages (503 / overloaded) that we want
/// to wait out rather than burn retry attempts on.
fn is_upstream_outage(error_msg: &str) -> bool {
    let s = error_msg.to_ascii_lowercase();
    s.contains("503")
        || s.contains("service unavailable")
        || s.contains("overloaded")
        || s.contains("unavailable")
}

async fn fail_job(
    pool: &SqlitePool,
    job: &Job,
    error_msg: &str,
    max_attempts: i32,
    non_fatal: bool,
) -> Result<()> {
    // For upstream provider outages, defer 30min without consuming an attempt.
    if is_upstream_outage(error_msg) {
        sqlx::query(
            "UPDATE jobs SET status = 'queued',
             run_after = datetime('now', '+1800 seconds')
             WHERE id = $1",
        )
        .bind(&job.id)
        .execute(pool)
        .await?;
        tracing::warn!(
            "Job {} hit upstream outage, deferring 30min (attempt {} unchanged): {error_msg}",
            job.id,
            job.attempts,
        );
        return Ok(());
    }

    let new_attempts = job.attempts + 1;
    let mut tx = pool.begin().await?;

    if new_attempts < max_attempts {
        let backoff_secs = 60 * (1 << new_attempts); // 2min, 4min, 8min...
        sqlx::query(
            "UPDATE jobs SET status = 'queued', attempts = $1,
             run_after = datetime('now', '+' || $2 || ' seconds')
             WHERE id = $3",
        )
        .bind(new_attempts)
        .bind(backoff_secs)
        .bind(&job.id)
        .execute(&mut *tx)
        .await?;

        tracing::warn!(
            "Job {} attempt {}/{} failed, retrying in {backoff_secs}s: {error_msg}",
            job.id,
            new_attempts,
            max_attempts,
        );
    } else {
        sqlx::query("UPDATE jobs SET status = 'error', attempts = $1 WHERE id = $2")
            .bind(new_attempts)
            .bind(&job.id)
            .execute(&mut *tx)
            .await?;

        // Image/describe failures don't set episode to error
        if !non_fatal {
            sqlx::query("UPDATE episodes SET status = 'error', error_msg = $1 WHERE id = $2")
                .bind(error_msg)
                .bind(&job.episode_id)
                .execute(&mut *tx)
                .await?;
        }

        tracing::error!(
            "Job {} permanently failed after {max_attempts} attempts: {error_msg}",
            job.id,
        );
    }

    tx.commit().await?;
    Ok(())
}
