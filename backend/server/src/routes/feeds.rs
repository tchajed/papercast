use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use crate::error::{AppError, AppResult};
use crate::ids::{new_id, new_token};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/feeds", post(create_feed).get(list_feeds))
        .route(
            "/api/v1/feeds/{feed_token}",
            get(get_feed).delete(delete_feed).patch(update_feed),
        )
        .route(
            "/api/v1/feeds/{feed_token}/image",
            post(regenerate_feed_image),
        )
}

#[derive(Debug, Deserialize)]
pub struct UpdateFeedRequest {
    pub slug: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateFeedRequest {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_tts")]
    pub tts_default: String,
}

fn default_tts() -> String {
    "google".into()
}

#[derive(Debug, Serialize, FromRow)]
pub struct FeedRow {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub feed_token: String,
    pub tts_default: String,
    pub created_at: String,
    pub image_url: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FeedResponse {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub feed_token: String,
    pub tts_default: String,
    pub rss_url: String,
    pub created_at: String,
    pub image_url: Option<String>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct FeedListItem {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub feed_token: String,
    pub tts_default: String,
    pub created_at: String,
    pub image_url: Option<String>,
    pub episode_count: i32,
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

async fn create_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateFeedRequest>,
) -> AppResult<(StatusCode, Json<FeedResponse>)> {
    require_admin(&headers, &state.config.admin_token)?;

    if req.tts_default != "google" {
        return Err(AppError::BadRequest(
            "tts_default must be 'google'".into(),
        ));
    }

    let id = new_id();
    let feed_token = new_token();

    sqlx::query(
        "INSERT INTO feeds (id, slug, title, description, feed_token, tts_default)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(&id)
    .bind(&req.slug)
    .bind(&req.title)
    .bind(&req.description)
    .bind(&feed_token)
    .bind(&req.tts_default)
    .execute(&state.pool)
    .await?;

    let rss_url = format!("{}/feed/{}/rss.xml", state.config.public_url, feed_token);

    let row = sqlx::query_as::<_, FeedRow>("SELECT * FROM feeds WHERE id = $1")
        .bind(&id)
        .fetch_one(&state.pool)
        .await?;

    Ok((
        StatusCode::CREATED,
        Json(FeedResponse {
            id: row.id,
            slug: row.slug,
            title: row.title,
            description: row.description,
            feed_token: row.feed_token,
            tts_default: row.tts_default,
            rss_url,
            created_at: row.created_at,
            image_url: row.image_url,
        }),
    ))
}

async fn list_feeds(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<Vec<FeedListItem>>> {
    require_admin(&headers, &state.config.admin_token)?;

    let feeds = sqlx::query_as::<_, FeedListItem>(
        "SELECT f.id, f.slug, f.title, f.description, f.feed_token, f.tts_default, f.created_at,
                f.image_url,
                CAST(COUNT(e.id) AS INTEGER) as episode_count
         FROM feeds f
         LEFT JOIN episodes e ON e.feed_id = f.id
         GROUP BY f.id
         ORDER BY f.created_at DESC",
    )
    .fetch_all(&state.pool)
    .await?;

    Ok(Json(feeds))
}

#[derive(Debug, Serialize)]
pub struct FeedWithEpisodes {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub description: String,
    pub tts_default: String,
    pub rss_url: String,
    pub image_url: Option<String>,
    pub episodes: Vec<EpisodeSummary>,
}

#[derive(Debug, Serialize, FromRow)]
pub struct EpisodeSummary {
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

async fn get_feed(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
) -> AppResult<Json<FeedWithEpisodes>> {
    let feed = sqlx::query_as::<_, FeedRow>("SELECT * FROM feeds WHERE feed_token = $1")
        .bind(&feed_token)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let episodes = sqlx::query_as::<_, EpisodeSummary>(
        "SELECT e.id, e.title, e.source_url, e.source_type, e.status, e.audio_url, e.image_url,
                e.duration_secs, e.word_count, e.tts_chunks_done, e.tts_chunks_total,
                e.tts_provider, e.description, e.error_msg, e.pub_date, e.created_at, e.summarize,
                (SELECT j.run_after FROM jobs j
                 WHERE j.episode_id = e.id AND j.status = 'queued'
                       AND j.run_after > datetime('now')
                 ORDER BY j.run_after ASC LIMIT 1) AS retry_at
         FROM episodes e WHERE e.feed_id = $1
         ORDER BY e.created_at DESC
         LIMIT 100",
    )
    .bind(&feed.id)
    .fetch_all(&state.pool)
    .await?;

    let rss_url = format!(
        "{}/feed/{}/rss.xml",
        state.config.public_url, feed.feed_token
    );

    Ok(Json(FeedWithEpisodes {
        id: feed.id,
        slug: feed.slug,
        title: feed.title,
        description: feed.description,
        tts_default: feed.tts_default,
        rss_url,
        image_url: feed.image_url,
        episodes,
    }))
}

async fn update_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(feed_token): Path<String>,
    Json(req): Json<UpdateFeedRequest>,
) -> AppResult<Json<FeedResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    let feed = sqlx::query_as::<_, FeedRow>("SELECT * FROM feeds WHERE feed_token = $1")
        .bind(&feed_token)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let new_slug = req.slug.unwrap_or(feed.slug);
    let new_title = req.title.unwrap_or(feed.title);
    let new_description = req.description.unwrap_or(feed.description);

    sqlx::query(
        "UPDATE feeds SET slug = $1, title = $2, description = $3 WHERE id = $4",
    )
    .bind(&new_slug)
    .bind(&new_title)
    .bind(&new_description)
    .bind(&feed.id)
    .execute(&state.pool)
    .await?;

    let rss_url = format!("{}/feed/{}/rss.xml", state.config.public_url, feed.feed_token);

    Ok(Json(FeedResponse {
        id: feed.id,
        slug: new_slug,
        title: new_title,
        description: new_description,
        feed_token: feed.feed_token,
        tts_default: feed.tts_default,
        rss_url,
        created_at: feed.created_at,
        image_url: feed.image_url,
    }))
}

async fn regenerate_feed_image(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(feed_token): Path<String>,
) -> AppResult<Json<FeedResponse>> {
    require_admin(&headers, &state.config.admin_token)?;

    let feed = sqlx::query_as::<_, FeedRow>("SELECT * FROM feeds WHERE feed_token = $1")
        .bind(&feed_token)
        .fetch_optional(&state.pool)
        .await?
        .ok_or(AppError::NotFound)?;

    let brief = if feed.description.trim().is_empty() {
        feed.title.clone()
    } else {
        format!("{}. {}", feed.title, feed.description)
    };

    let image = tts_lib::image::generate_feed_cover(
        &state.config.google_studio_api_key,
        &brief,
    )
    .await?;

    let image_url = state
        .storage
        .upload_feed_image(&feed.id, image.bytes, &image.mime_type)
        .await?;

    sqlx::query("UPDATE feeds SET image_url = $1 WHERE id = $2")
        .bind(&image_url)
        .bind(&feed.id)
        .execute(&state.pool)
        .await?;

    let rss_url = format!("{}/feed/{}/rss.xml", state.config.public_url, feed.feed_token);

    Ok(Json(FeedResponse {
        id: feed.id,
        slug: feed.slug,
        title: feed.title,
        description: feed.description,
        feed_token: feed.feed_token,
        tts_default: feed.tts_default,
        rss_url,
        created_at: feed.created_at,
        image_url: Some(image_url),
    }))
}

async fn delete_feed(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(feed_token): Path<String>,
) -> AppResult<StatusCode> {
    require_admin(&headers, &state.config.admin_token)?;

    let result = sqlx::query("DELETE FROM feeds WHERE feed_token = $1")
        .bind(&feed_token)
        .execute(&state.pool)
        .await?;

    if result.rows_affected() == 0 {
        return Err(AppError::NotFound);
    }

    Ok(StatusCode::NO_CONTENT)
}
