use anyhow::{Context, Result};

use crate::config::AppConfig;

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let (source_url, source_type) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT source_url, source_type FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let source_url = source_url.context("No source_url for scrape stage")?;

    match tts_lib::scrape::scrape(&source_url, &source_type).await {
        Ok(doc) => {
            let title = doc.title.as_deref().unwrap_or(&source_url);
            let raw_text = doc
                .raw_text
                .as_ref()
                .context("No text extracted from URL")?;

            sqlx::query("UPDATE episodes SET title = $1, raw_text = $2 WHERE id = $3")
                .bind(title)
                .bind(raw_text)
                .bind(episode_id)
                .execute(pool)
                .await?;

            Ok(())
        }
        Err(e) if source_type == "arxiv" => {
            tracing::warn!(
                "arxiv scrape failed for {episode_id} ({source_url}): {e:#}; falling back to PDF"
            );
            fallback_to_pdf(episode_id, &source_url, pool, config).await
        }
        Err(e) => Err(e),
    }
}

async fn fallback_to_pdf(
    episode_id: &str,
    source_url: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let arxiv_id = tts_lib::scrape::extract_arxiv_id(source_url)
        .context("Could not extract arxiv ID for PDF fallback")?;
    let pdf_url = format!("https://arxiv.org/pdf/{arxiv_id}");

    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let bytes = client
        .get(&pdf_url)
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch arxiv PDF")?
        .bytes()
        .await?;

    let pdf_path = format!("/data/{}.pdf", episode_id);
    tokio::fs::write(&pdf_path, &bytes)
        .await
        .context("Failed to write arxiv PDF")?;

    crate::pipeline::pdf::run(episode_id, pool, config).await
}
