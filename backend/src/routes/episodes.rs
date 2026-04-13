use axum::{
    extract::{Multipart, Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/feeds/{feed_token}/episodes",
            post(submit_episode),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/pdf",
            post(upload_pdf),
        )
        .route(
            "/api/v1/feeds/{feed_token}/episodes/{episode_id}",
            get(get_episode).delete(delete_episode),
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
    pub error_msg: Option<String>,
    pub pub_date: Option<String>,
    pub created_at: String,
}

fn detect_source_type(url: &str) -> &'static str {
    if url.contains("arxiv.org/abs/") || url.contains("ar5iv.org") {
        "arxiv"
    } else {
        "article"
    }
}

fn extract_arxiv_id(url: &str) -> Option<String> {
    let patterns = ["arxiv.org/abs/", "ar5iv.org/abs/"];
    for pat in patterns {
        if let Some(idx) = url.find(pat) {
            let rest = &url[idx + pat.len()..];
            let id: String = rest.chars().take_while(|c| *c != '/' && *c != '?').collect();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

/// Resolve the feed ID from a feed_token, returning NotFound if invalid.
async fn resolve_feed(pool: &sqlx::SqlitePool, feed_token: &str) -> AppResult<String> {
    let row =
        sqlx::query_scalar::<_, String>("SELECT id FROM feeds WHERE feed_token = $1")
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

async fn submit_episode(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
    Json(req): Json<SubmitEpisodeRequest>,
) -> AppResult<(StatusCode, Json<SubmitEpisodeResponse>)> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;
    let source_type = detect_source_type(&req.url);
    let default = get_tts_default(&state.pool, &feed_id).await?;
    let tts_provider = validate_tts_provider(req.tts_provider.as_ref(), default)?;

    let title = if source_type == "arxiv" {
        extract_arxiv_id(&req.url)
            .map(|id| format!("arXiv:{id}"))
            .unwrap_or_else(|| req.url.clone())
    } else {
        req.url.clone()
    };

    let episode_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO episodes (id, feed_id, title, source_url, source_type, tts_provider, status)
         VALUES ($1, $2, $3, $4, $5, $6, 'pending')",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .bind(&title)
    .bind(&req.url)
    .bind(source_type)
    .bind(&tts_provider)
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
    let mut title: Option<String> = None;
    let mut tts_provider_field: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        AppError::BadRequest(format!("Failed to read multipart field: {e}"))
    })? {
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
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
                tts_provider_field = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| {
                            AppError::BadRequest(format!("Failed to read tts_provider: {e}"))
                        })?,
                );
            }
            _ => {}
        }
    }

    let pdf_bytes = pdf_bytes.ok_or_else(|| AppError::BadRequest("No file field".into()))?;
    let default = get_tts_default(&state.pool, &feed_id).await?;
    let tts_provider = validate_tts_provider(tts_provider_field.as_ref(), default)?;
    let title = title.unwrap_or_else(|| "PDF Upload".into());

    let episode_id = Uuid::new_v4().to_string();
    let job_id = Uuid::new_v4().to_string();

    // Write PDF to temp file for the pdf pipeline stage
    let pdf_path = format!("/tmp/{}.pdf", episode_id);
    tokio::fs::write(&pdf_path, &pdf_bytes)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to write temp PDF: {e}"))?;

    let mut tx = state.pool.begin().await?;

    sqlx::query(
        "INSERT INTO episodes (id, feed_id, title, source_type, tts_provider, status)
         VALUES ($1, $2, $3, 'pdf', $4, 'pending')",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .bind(&title)
    .bind(&tts_provider)
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
            source_url: None,
            source_type: "pdf".into(),
        }),
    ))
}

async fn get_episode(
    State(state): State<AppState>,
    Path((feed_token, episode_id)): Path<(String, String)>,
) -> AppResult<Json<EpisodeResponse>> {
    let feed_id = resolve_feed(&state.pool, &feed_token).await?;

    let ep = sqlx::query_as::<_, EpisodeResponse>(
        "SELECT id, title, source_url, source_type, status, audio_url, image_url,
                duration_secs, word_count, tts_chunks_done, tts_chunks_total,
                tts_provider, error_msg, pub_date, created_at
         FROM episodes WHERE id = $1 AND feed_id = $2",
    )
    .bind(&episode_id)
    .bind(&feed_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

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
        assert_eq!(detect_source_type("https://arxiv.org/abs/2301.12345"), "arxiv");
    }

    #[test]
    fn test_detect_source_type_ar5iv() {
        assert_eq!(detect_source_type("https://ar5iv.org/abs/2301.12345"), "arxiv");
    }

    #[test]
    fn test_detect_source_type_article() {
        assert_eq!(detect_source_type("https://example.com/some-article"), "article");
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
        "tts" => "cleaning",
        _ => "pending",
    };

    let job_id = Uuid::new_v4().to_string();
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
