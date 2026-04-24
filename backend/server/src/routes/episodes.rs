use crate::error::{AppError, AppResult};
use crate::ids::new_id;
use crate::AppState;
use axum::{
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/feeds/{feed_token}/episodes", post(submit_episode))
        .route(
            "/api/v1/feeds/{feed_token}/episodes/pdf",
            post(upload_pdf).layer(DefaultBodyLimit::max(50 * 1024 * 1024)),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/text",
            post(submit_text).layer(DefaultBodyLimit::max(10 * 1024 * 1024)),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/{episode_id}",
            get(get_episode)
                .delete(delete_episode)
                .patch(update_episode),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/{episode_id}/text",
            get(get_episode_text),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/{episode_id}/retry",
            post(retry_episode),
        )
}

#[derive(Debug, Deserialize)]
pub struct SubmitEpisodeRequest {
    pub url: String,
    pub tts_provider: Option<String>,
    pub tts_voice: Option<String>,
    #[serde(default)]
    pub summarize: bool,
    pub summarize_focus: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateEpisodeRequest {
    pub title: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SubmitEpisodeResponse {
    pub id: String,
    pub status: String,
    pub source_url: Option<String>,
    pub source_type: String,
}

#[derive(Debug, Serialize, FromRow)]
pub struct EpisodeResponse {
    pub id: String,
    pub title: String,
    pub source_url: Option<String>,
    pub source_type: String,
    pub status: String,
    pub audio_url: Option<String>,
    pub image_url: Option<String>,
    pub duration_secs: Option<i32>,
    pub word_count: Option<i32>,
    pub tts_chunks_done: i32,
    pub tts_chunks_total: i32,
    pub tts_provider: Option<String>,
    pub description: Option<String>,
    pub error_msg: Option<String>,
    pub pub_date: Option<String>,
    pub created_at: String,
    pub summarize: i32,
    pub retry_at: Option<String>,
}

fn detect_source_type(url: &str) -> &'static str {
    if url.contains("arxiv.org/abs/")
        || url.contains("arxiv.org/html/")
        || url.contains("arxiv.org/pdf/")
        || url.contains("ar5iv.org")
    {
        "arxiv"
    } else if tts_lib::scrape::url_looks_like_pdf(url) {
        "pdf"
    } else {
        "article"
    }
}

fn extract_arxiv_id(url: &str) -> Option<String> {
    let patterns = ["arxiv.org/abs/", "ar5iv.org/abs/"];
    for pat in patterns {
        if let Some(idx) = url.find(pat) {
            let rest = &url[idx + pat.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| *c != '/' && *c != '?')
                .collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

/// Resolve the feed ID from a feed_token, returning NotFound if invalid.
async fn resolve_feed(pool: &sqlx::SqlitePool, feed_token: &str) -> AppResult<String> {
    let row = sqlx::query_scalar::<_, String>("SELECT id FROM feeds WHERE feed_token = $1")
        .bind(feed_token)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(row)
}

async fn get_tts_default(pool: &sqlx::SqlitePool, feed_id: &str) -> AppResult<String> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT tts_default FROM feeds WHERE id = $1")
            .bind(feed_id)
            .fetch_one(pool)
            .await?,
    )
}

fn validate_tts_provider(provider: Option<&String>, default: String) -> AppResult<String> {
    match provider {
        Some(p) if p == "google" => Ok(p.clone()),
        Some(p) => Err(AppError::BadRequest(format!("Invalid tts_provider: {p}"))),
        None => Ok(default),
    }
}

/// Voice IDs the UI is allowed to select. Keep in sync with the frontend
/// VOICES catalog in src/lib/voices.ts. Unknown voices are rejected at the
/// API boundary so a typo doesn't reach Google TTS as an opaque 400.
const SUPPORTED_VOICES: &[&str] = &["en-US-Chirp3-HD-Puck", "en-US-Chirp3-HD-Kore"];

fn validate_tts_voice(voice: Option<&String>) -> AppResult<Option<String>> {
    match voice.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(v) if SUPPORTED_VOICES.contains(&v) => Ok(Some(v.to_string())),
        Some(v) => Err(AppError::BadRequest(format!("Invalid tts_voice: {v}"))),
        None => Ok(None),
    }
}

async fn submit_episode(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
    Json(req): Json<SubmitEpisodeRequest>,
) -> AppResult<(StatusCode, Json<SubmitEpisodeResponse>)> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;
    let source_type = detect_source_type(&req.url);
    let default = get_tts_default(&state.pool, &feed_id).await?;
    let tts_provider = validate_tts_provider(req.tts_provider.as_ref(), default)?;
    let tts_voice = validate_tts_voice(req.tts_voice.as_ref())?;

    let title = if source_type == "arxiv" {
        extract_arxiv_id(&req.url)
            .map(|id| format!("arXiv:{id}"))
            .unwrap_or_else(|| req.url.clone())
    } else {
        req.url.clone()
    };

    let episode_id = new_id();
    let job_id = new_id();

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO episodes (id, feed_id, title, source_url, source_type, tts_provider, tts_voice, summarize, summarize_focus, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'pending')",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .bind(&title)
    .bind(&req.url)
    .bind(source_type)
    .bind(&tts_provider)
    .bind(&tts_voice)
    .bind(req.summarize as i32)
    .bind(req.summarize_focus.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'scrape', 'queued')",
    )
    .bind(&job_id)
    .bind(&episode_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitEpisodeResponse {
            id: episode_id,
            status: "pending".into(),
            source_url: Some(req.url),
            source_type: source_type.into(),
        }),
    ))
}

async fn upload_pdf(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<SubmitEpisodeResponse>)> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let mut pdf_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut title: Option<String> = None;
    let mut tts_provider_field: Option<String> = None;
    let mut tts_voice_field: Option<String> = None;
    let mut source_url: Option<String> = None;
    let mut summarize = false;
    let mut summarize_focus: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("Failed to read multipart field: {e}")))?
    {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().map(|s| s.to_string());
                pdf_bytes = Some(
                    field
                        .bytes()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Failed to read file: {e}")))?
                        .to_vec(),
                );
            }
            "title" => {
                title = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("Failed to read title: {e}")))?,
                );
            }
            "tts_provider" => {
                tts_provider_field = Some(field.text().await.map_err(|e| {
                    AppError::BadRequest(format!("Failed to read tts_provider: {e}"))
                })?);
            }
            "tts_voice" => {
                tts_voice_field =
                    Some(field.text().await.map_err(|e| {
                        AppError::BadRequest(format!("Failed to read tts_voice: {e}"))
                    })?);
            }
            "summarize" => {
                let val = field.text().await.unwrap_or_default();
                summarize = val == "true" || val == "1";
            }
            "source_url" => {
                let val = field
                    .text()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("Failed to read source_url: {e}")))?;
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    source_url = Some(trimmed.to_string());
                }
            }
            "summarize_focus" => {
                let val = field.text().await.unwrap_or_default();
                let trimmed = val.trim();
                if !trimmed.is_empty() {
                    summarize_focus = Some(trimmed.to_string());
                }
            }
            _ => {}
        }
    }

    let file_bytes = pdf_bytes.ok_or_else(|| AppError::BadRequest("No file field".into()))?;
    let default = get_tts_default(&state.pool, &feed_id).await?;
    let tts_provider = validate_tts_provider(tts_provider_field.as_ref(), default)?;
    let tts_voice = validate_tts_voice(tts_voice_field.as_ref())?;

    let is_markdown = file_name
        .as_deref()
        .map(|n| {
            let lower = n.to_ascii_lowercase();
            lower.ends_with(".md") || lower.ends_with(".markdown") || lower.ends_with(".txt")
        })
        .unwrap_or(false);

    let episode_id = new_id();
    let job_id = new_id();

    if is_markdown {
        let text = String::from_utf8(file_bytes)
            .map_err(|_| AppError::BadRequest("Markdown upload must be valid UTF-8".into()))?;
        let title = title.unwrap_or_else(|| {
            file_name
                .as_deref()
                .and_then(|n| n.rsplit('/').next())
                .map(|n| {
                    n.trim_end_matches(".md")
                        .trim_end_matches(".markdown")
                        .trim_end_matches(".txt")
                        .to_string()
                })
                .unwrap_or_else(|| "Markdown Upload".into())
        });

        let mut tx = state.pool.begin().await?;
        sqlx::query(
            "INSERT INTO episodes (id, feed_id, title, source_url, source_type, raw_text, tts_provider, tts_voice, summarize, summarize_focus, status)
             VALUES ($1, $2, $3, $4, 'markdown', $5, $6, $7, $8, $9, 'cleaning')",
        )
        .bind(&episode_id)
        .bind(&feed_id)
        .bind(&title)
        .bind(&source_url)
        .bind(&text)
        .bind(&tts_provider)
        .bind(&tts_voice)
        .bind(summarize as i32)
        .bind(&summarize_focus)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'clean', 'queued')",
        )
        .bind(&job_id)
        .bind(&episode_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        return Ok((
            StatusCode::ACCEPTED,
            Json(SubmitEpisodeResponse {
                id: episode_id,
                status: "cleaning".into(),
                source_url,
                source_type: "markdown".into(),
            }),
        ));
    }

    let title = title.unwrap_or_else(|| "PDF Upload".into());

    // Write PDF to temp file for the pdf pipeline stage
    let pdf_path = format!("/data/{}.pdf", episode_id);
    tokio::fs::write(&pdf_path, &file_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp PDF: {e}"))?;

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO episodes (id, feed_id, title, source_url, source_type, tts_provider, tts_voice, summarize, summarize_focus, status)
         VALUES ($1, $2, $3, $4, 'pdf', $5, $6, $7, $8, 'pending')",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .bind(&title)
    .bind(&source_url)
    .bind(&tts_provider)
    .bind(&tts_voice)
    .bind(summarize as i32)
    .bind(&summarize_focus)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'pdf', 'queued')",
    )
    .bind(&job_id)
    .bind(&episode_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitEpisodeResponse {
            id: episode_id,
            status: "pending".into(),
            source_url,
            source_type: "pdf".into(),
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct SubmitTextRequest {
    pub title: String,
    pub text: String,
    pub source_url: Option<String>,
    pub tts_provider: Option<String>,
    pub tts_voice: Option<String>,
    #[serde(default)]
    pub summarize: bool,
    pub summarize_focus: Option<String>,
}

async fn submit_text(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
    Json(req): Json<SubmitTextRequest>,
) -> AppResult<(StatusCode, Json<SubmitEpisodeResponse>)> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;
    if req.title.trim().is_empty() {
        return Err(AppError::BadRequest("title is required".into()));
    }
    if req.text.trim().is_empty() {
        return Err(AppError::BadRequest("text is required".into()));
    }
    let default = get_tts_default(&state.pool, &feed_id).await?;
    let tts_provider = validate_tts_provider(req.tts_provider.as_ref(), default)?;
    let tts_voice = validate_tts_voice(req.tts_voice.as_ref())?;
    let source_url = req
        .source_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let episode_id = new_id();
    let job_id = new_id();

    let mut tx = state.pool.begin().await?;

    // Markdown/text goes in as raw_text directly; we skip the scrape stage
    // and enqueue a clean job to normalize it for TTS.
    let focus = req
        .summarize_focus
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);
    sqlx::query(
        "INSERT INTO episodes (id, feed_id, title, source_url, source_type, raw_text, tts_provider, tts_voice, summarize, summarize_focus, status)
         VALUES ($1, $2, $3, $4, 'markdown', $5, $6, $7, $8, $9, 'cleaning')",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .bind(&req.title)
    .bind(&source_url)
    .bind(&req.text)
    .bind(&tts_provider)
    .bind(&tts_voice)
    .bind(req.summarize as i32)
    .bind(&focus)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, 'clean', 'queued')",
    )
    .bind(&job_id)
    .bind(&episode_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitEpisodeResponse {
            id: episode_id,
            status: "cleaning".into(),
            source_url,
            source_type: "markdown".into(),
        }),
    ))
}

async fn get_episode(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
) -> AppResult<Json<EpisodeResponse>> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let ep = sqlx::query_as::<_, EpisodeResponse>(
        "SELECT e.id, e.title, e.source_url, e.source_type, e.status, e.audio_url, e.image_url,
                e.duration_secs, e.word_count, e.tts_chunks_done, e.tts_chunks_total,
                e.tts_provider, e.description, e.error_msg, e.pub_date, e.created_at, e.summarize,
                (SELECT j.run_after FROM jobs j
                 WHERE j.episode_id = e.id AND j.status = 'queued'
                       AND j.run_after > datetime('now')
                       AND j.job_type NOT IN ('image', 'describe')
                 ORDER BY j.run_after ASC LIMIT 1) AS retry_at
         FROM episodes e WHERE e.id = $1 AND e.feed_id = $2",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    Ok(Json(ep))
}

async fn update_episode(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
    Json(req): Json<UpdateEpisodeRequest>,
) -> AppResult<Json<EpisodeResponse>> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM episodes WHERE id = $1 AND feed_id = $2")
            .bind(&episode_id)
            .bind(&feed_id)
            .fetch_optional(&state.pool)
            .await?;
    if exists.is_none() {
        return Err(AppError::NotFound);
    }

    if let Some(title) = req.title.as_ref() {
        let t = title.trim();
        if t.is_empty() {
            return Err(AppError::BadRequest("title cannot be empty".into()));
        }
        sqlx::query("UPDATE episodes SET title = $1 WHERE id = $2 AND feed_id = $3")
            .bind(t)
            .bind(&episode_id)
            .bind(&feed_id)
            .execute(&state.pool)
            .await?;
    }

    if let Some(source_url) = req.source_url.as_ref() {
        let url = source_url.trim();
        let new_url: Option<String> = if url.is_empty() {
            None
        } else {
            Some(url.to_string())
        };
        let new_type = new_url.as_deref().map(detect_source_type);
        sqlx::query("UPDATE episodes SET source_url = $1, source_type = COALESCE($2, source_type) WHERE id = $3 AND feed_id = $4")
            .bind(&new_url)
            .bind(new_type)
            .bind(&episode_id)
            .bind(&feed_id)
            .execute(&state.pool)
            .await?;
    }

    let ep = sqlx::query_as::<_, EpisodeResponse>(
        "SELECT e.id, e.title, e.source_url, e.source_type, e.status, e.audio_url, e.image_url,
                e.duration_secs, e.word_count, e.tts_chunks_done, e.tts_chunks_total,
                e.tts_provider, e.description, e.error_msg, e.pub_date, e.created_at, e.summarize,
                (SELECT j.run_after FROM jobs j
                 WHERE j.episode_id = e.id AND j.status = 'queued'
                       AND j.run_after > datetime('now')
                       AND j.job_type NOT IN ('image', 'describe')
                 ORDER BY j.run_after ASC LIMIT 1) AS retry_at
         FROM episodes e WHERE e.id = $1 AND e.feed_id = $2",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .fetch_one(&state.pool)
    .await?;

    Ok(Json(ep))
}

async fn delete_episode(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let result = sqlx::query("DELETE FROM episodes WHERE id = $1 AND feed_id = $2")
        .bind(&episode_id)
        .bind(&feed_id)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_source_type_arxiv() {
        assert_eq!(
            detect_source_type("https://arxiv.org/abs/2301.12345"),
            "arxiv"
        );
    }

    #[test]
    fn test_detect_source_type_ar5iv() {
        assert_eq!(
            detect_source_type("https://ar5iv.org/abs/2301.12345"),
            "arxiv"
        );
    }

    #[test]
    fn test_detect_source_type_article() {
        assert_eq!(
            detect_source_type("https://example.com/some-article"),
            "article"
        );
    }

    #[test]
    fn test_extract_arxiv_id_standard() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_ar5iv() {
        assert_eq!(
            extract_arxiv_id("https://ar5iv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_with_version() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2301.12345v2"),
            Some("2301.12345v2".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_with_query() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2301.12345?context=cs"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_with_trailing_slash() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2301.12345/"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_no_match() {
        assert_eq!(extract_arxiv_id("https://example.com/article"), None);
    }

    #[test]
    fn test_validate_tts_provider_google() {
        let result = validate_tts_provider(Some(&"google".into()), "google".into());
        assert_eq!(result.unwrap(), "google");
    }

    #[test]
    fn test_validate_tts_provider_invalid() {
        let result = validate_tts_provider(Some(&"invalid".into()), "google".into());
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_tts_provider_none_uses_default() {
        let result = validate_tts_provider(None, "google".into());
        assert_eq!(result.unwrap(), "google");
    }
}

async fn get_episode_text(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let (cleaned_text, transcript, raw_text, sections_json) =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>, Option<String>)>(
            "SELECT cleaned_text, transcript, raw_text, sections_json FROM episodes WHERE id = $1 AND feed_id = $2",
        )
        .bind(&episode_id)
        .bind(&feed_id)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let sections = sections_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok());

    Ok(Json(serde_json::json!({
        "cleaned_text": cleaned_text,
        "transcript": transcript,
        "raw_text": raw_text,
        "sections": sections,
    })))
}

async fn retry_episode(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
) -> AppResult<Json<serde_json::Value>> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let (status, _error_msg) = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT status, error_msg FROM episodes WHERE id = $1 AND feed_id = $2",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    if status != "error" {
        return Err(AppError::BadRequest(
            "Can only retry episodes in error state".into(),
        ));
    }

    let failed_job_type = sqlx::query_scalar::<_, String>(
        "SELECT job_type FROM jobs WHERE episode_id = $1 AND status = 'error'
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(&episode_id)
    .fetch_optional(&state.pool)
    .await?
    .unwrap_or_else(|| "scrape".into());

    let new_status = match failed_job_type.as_str() {
        "scrape" | "pdf" => "pending",
        "clean" => "scraping",
        "summarize" => "cleaning",
        "tts" => "cleaning",
        _ => "pending",
    };

    let job_id = new_id();
    let mut tx = state.pool.begin().await?;

    sqlx::query("UPDATE episodes SET status = $1, error_msg = NULL WHERE id = $2")
        .bind(new_status)
        .bind(&episode_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query(
        "INSERT INTO jobs (id, episode_id, job_type, status) VALUES ($1, $2, $3, 'queued')",
    )
    .bind(&job_id)
    .bind(&episode_id)
    .bind(&failed_job_type)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(Json(serde_json::json!({
        "id": episode_id,
        "status": new_status,
        "retrying_stage": failed_job_type,
    })))
}
