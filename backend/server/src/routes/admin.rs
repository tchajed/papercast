//! Read-only admin endpoints (gated by ADMIN_TOKEN) for monitoring the
//! system: active jobs, TTS chunk progress, and AI/TTS usage totals.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use sqlx::FromRow;

use crate::error::{AppError, AppResult};
use crate::ids::new_id;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/admin/status", get(status))
        .route("/api/v1/admin/jobs", get(list_jobs))
        .route("/api/v1/admin/usage", get(usage_summary))
        .route("/api/v1/admin/usage/episode/{episode_id}", get(usage_for_episode))
        .route(
            "/api/v1/admin/episodes/{episode_id}/backfill-chapters",
            post(backfill_chapters),
        )
        .route(
            "/api/v1/admin/episodes/{episode_id}/rerun-tts",
            post(rerun_tts),
        )
}

fn require_admin(headers: &HeaderMap, admin_token: &str) -> AppResult<()> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth.strip_prefix("Bearer ").unwrap_or("");
    if token != admin_token {
        return Err(AppError::Unauthorized);
    }
    Ok(())
}

#[derive(Serialize, FromRow)]
struct JobRow {
    id: String,
    episode_id: String,
    episode_title: String,
    feed_slug: String,
    job_type: String,
    status: String,
    attempts: i32,
    run_after: String,
    created_at: String,
    tts_chunks_done: i32,
    tts_chunks_total: i32,
}

#[derive(Serialize)]
struct JobsResponse {
    jobs: Vec<JobRow>,
}

async fn list_jobs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<JobsResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    let jobs = sqlx::query_as::<_, JobRow>(
        "SELECT j.id, j.episode_id, e.title AS episode_title, f.slug AS feed_slug,
                j.job_type, j.status, j.attempts, j.run_after, j.created_at,
                e.tts_chunks_done, e.tts_chunks_total
         FROM jobs j
         JOIN episodes e ON e.id = j.episode_id
         JOIN feeds f ON f.id = e.feed_id
         WHERE j.status IN ('queued', 'running')
         ORDER BY
             CASE j.status WHEN 'running' THEN 0 ELSE 1 END,
             j.run_after ASC",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(JobsResponse { jobs }))
}

#[derive(Serialize, FromRow)]
struct StatusCounts {
    pending: i64,
    active: i64,
    error: i64,
    done: i64,
    queued_jobs: i64,
    running_jobs: i64,
}

async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<StatusCounts>> {
    require_admin(&headers, &state.config.admin_token)?;

    let row = sqlx::query_as::<_, StatusCounts>(
        "SELECT
            (SELECT COUNT(*) FROM episodes WHERE status = 'pending') AS pending,
            (SELECT COUNT(*) FROM episodes WHERE status IN ('scraping','cleaning','summarizing','tts')) AS active,
            (SELECT COUNT(*) FROM episodes WHERE status = 'error') AS error,
            (SELECT COUNT(*) FROM episodes WHERE status = 'done') AS done,
            (SELECT COUNT(*) FROM jobs WHERE status = 'queued') AS queued_jobs,
            (SELECT COUNT(*) FROM jobs WHERE status = 'running') AS running_jobs",
    )
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(row))
}

#[derive(Serialize, FromRow)]
struct UsageGroup {
    provider: String,
    model: String,
    stage: String,
    calls: i64,
    input_tokens: i64,
    output_tokens: i64,
}

#[derive(Serialize)]
struct UsageSummaryResponse {
    since: Option<String>,
    groups: Vec<UsageGroup>,
    total_estimated_usd: f64,
}

/// Cost estimator. Pricing is approximate and meant for relative comparison,
/// not billing. Units: USD per 1M tokens (or per 1M chars for TTS).
fn estimated_cost_usd(u: &UsageGroup) -> f64 {
    let (in_rate, out_rate) = match (u.provider.as_str(), u.model.as_str()) {
        ("claude", m) if m.contains("opus") => (15.0, 75.0),
        ("claude", m) if m.contains("sonnet") => (3.0, 15.0),
        ("claude", m) if m.contains("haiku") => (1.0, 5.0),
        ("claude", _) => (3.0, 15.0),
        ("gemini", m) if m.contains("image") => (0.30, 30.0), // image model — output tokens reported as image tokens
        ("gemini", _) => (0.30, 2.50),
        ("google_tts", m) if m.contains("Chirp3-HD") => (30.0, 0.0), // per 1M chars
        ("google_tts", m) if m.contains("Studio") => (160.0, 0.0),   // per 1M chars
        ("google_tts", _) => (16.0, 0.0), // per 1M chars, Neural2/WaveNet
        _ => (0.0, 0.0),
    };
    (u.input_tokens as f64 * in_rate + u.output_tokens as f64 * out_rate) / 1_000_000.0
}

async fn usage_summary(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> AppResult<Json<UsageSummaryResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    // Optional `days` window (default: all time).
    let days: Option<i64> = params.get("days").and_then(|s| s.parse().ok());
    let (sql, since) = if let Some(d) = days {
        (
            "SELECT provider, model, stage, COUNT(*) AS calls,
                    SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens
             FROM ai_usage
             WHERE created_at >= datetime('now', '-' || $1 || ' days')
             GROUP BY provider, model, stage
             ORDER BY provider, model, stage"
                .to_string(),
            Some(format!("{d} days")),
        )
    } else {
        (
            "SELECT provider, model, stage, COUNT(*) AS calls,
                    SUM(input_tokens) AS input_tokens, SUM(output_tokens) AS output_tokens
             FROM ai_usage
             GROUP BY provider, model, stage
             ORDER BY provider, model, stage"
                .to_string(),
            None,
        )
    };

    let groups: Vec<UsageGroup> = if days.is_some() {
        sqlx::query_as(&sql).bind(days.unwrap()).fetch_all(&state.pool).await?
    } else {
        sqlx::query_as(&sql).fetch_all(&state.pool).await?
    };

    let total_estimated_usd: f64 = groups.iter().map(estimated_cost_usd).sum();

    Ok(Json(UsageSummaryResponse {
        since,
        groups,
        total_estimated_usd,
    }))
}

#[derive(Serialize, FromRow)]
struct EpisodeUsageRow {
    stage: String,
    provider: String,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    created_at: String,
}

#[derive(Serialize)]
struct EpisodeUsageResponse {
    episode_id: String,
    rows: Vec<EpisodeUsageRow>,
    estimated_usd: f64,
}

async fn usage_for_episode(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(episode_id): Path<String>,
) -> AppResult<(StatusCode, Json<EpisodeUsageResponse>)> {
    require_admin(&headers, &state.config.admin_token)?;

    let rows = sqlx::query_as::<_, EpisodeUsageRow>(
        "SELECT stage, provider, model, input_tokens, output_tokens, created_at
         FROM ai_usage WHERE episode_id = $1 ORDER BY created_at ASC",
    )
    .bind(&episode_id)
    .fetch_all(&state.pool)
    .await?;

    let estimated_usd: f64 = rows
        .iter()
        .map(|r| {
            estimated_cost_usd(&UsageGroup {
                provider: r.provider.clone(),
                model: r.model.clone(),
                stage: r.stage.clone(),
                calls: 1,
                input_tokens: r.input_tokens,
                output_tokens: r.output_tokens,
            })
        })
        .sum();

    Ok((
        StatusCode::OK,
        Json(EpisodeUsageResponse {
            episode_id,
            rows,
            estimated_usd,
        }),
    ))
}

#[derive(Serialize)]
struct BackfillChaptersResponse {
    episode_id: String,
    old_audio_url: String,
    new_audio_url: String,
    chapters: usize,
    audio_bytes: i64,
}

async fn backfill_chapters(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(episode_id): Path<String>,
) -> AppResult<Json<BackfillChaptersResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    let (audio_url, sections_json, duration_secs) =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<i32>)>(
            "SELECT audio_url, sections_json, duration_secs FROM episodes WHERE id = $1",
        )
        .bind(&episode_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let audio_url = audio_url.ok_or_else(|| AppError::BadRequest("episode has no audio_url".into()))?;
    let sections_json = sections_json
        .ok_or_else(|| AppError::BadRequest("episode has no sections_json".into()))?;
    let sections: Vec<tts_lib::tts::Section> = serde_json::from_str(&sections_json)
        .map_err(|e| AppError::BadRequest(format!("invalid sections_json: {e}")))?;
    if sections.is_empty() {
        return Err(AppError::BadRequest("episode has no chapters".into()));
    }
    let duration = duration_secs.unwrap_or(0) as u32;

    let resp = reqwest::get(&audio_url)
        .await
        .map_err(|e| anyhow::anyhow!("fetch existing mp3: {e}"))?;
    if !resp.status().is_success() {
        return Err(AppError::BadRequest(format!(
            "fetch existing mp3: status {}",
            resp.status()
        )));
    }
    let existing = resp
        .bytes()
        .await
        .map_err(|e| anyhow::anyhow!("read mp3 body: {e}"))?;

    let with_chapters =
        tts_lib::tts::embed_chapters(&existing, &sections, duration)?;
    let audio_bytes = with_chapters.len() as i64;
    let new_audio_url = state
        .storage
        .upload_episode_audio(&episode_id, with_chapters)
        .await?;

    sqlx::query("UPDATE episodes SET audio_url = $1, audio_bytes = $2 WHERE id = $3")
        .bind(&new_audio_url)
        .bind(audio_bytes)
        .bind(&episode_id)
        .execute(&state.pool)
        .await?;

    if new_audio_url != audio_url {
        if let Err(e) = state.storage.delete_object(&audio_url).await {
            tracing::warn!("failed to delete old audio {audio_url}: {e}");
        }
    }

    Ok(Json(BackfillChaptersResponse {
        episode_id,
        old_audio_url: audio_url,
        new_audio_url,
        chapters: sections.len(),
        audio_bytes,
    }))
}

#[derive(Serialize)]
struct RerunTtsResponse {
    episode_id: String,
    job_id: String,
}

/// Re-synthesize an existing episode's audio by enqueueing a fresh `tts` job.
/// Requires the episode to already have `cleaned_text` (or `transcript`) —
/// this is a TTS-only rerun, not a full pipeline restart.
async fn rerun_tts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(episode_id): Path<String>,
) -> AppResult<Json<RerunTtsResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    let (has_text,) = sqlx::query_as::<_, (bool,)>(
        "SELECT (cleaned_text IS NOT NULL OR transcript IS NOT NULL) AS has_text
         FROM episodes WHERE id = $1",
    )
    .bind(&episode_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if !has_text {
        return Err(AppError::BadRequest(
            "episode has no cleaned_text or transcript; run full pipeline instead".into(),
        ));
    }

    let job_id = new_id();
    let mut tx = state.pool.begin().await?;

    // Reset progress so the UI doesn't show stale "N/N complete" during the rerun.
    sqlx::query(
        "UPDATE episodes SET status = 'tts', error_msg = NULL,
             tts_chunks_done = 0, tts_chunks_total = 0
         WHERE id = $1",
    )
    .bind(&episode_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'tts', 'queued')",
    )
    .bind(&job_id)
    .bind(&episode_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(RerunTtsResponse { episode_id, job_id }))
}
