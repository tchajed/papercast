use anyhow::{Context, Result};

use crate::config::AppConfig;

pub async fn run(episode_id: &str, pool: &sqlx::SqlitePool, config: &AppConfig) -> Result<()> {
    let (source_url, source_type) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT source_url, source_type FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let source_url = source_url.context("No source_url for scrape stage")?;

    if source_type == "arxiv" {
        return match tts_lib::scrape::scrape(&source_url, &source_type).await {
            Ok(doc) => save_scraped(episode_id, &source_url, doc, pool).await,
            Err(e) => {
                tracing::warn!(
                    "arxiv scrape failed for {episode_id} ({source_url}): {e:#}; falling back to PDF"
                );
                let arxiv_id = tts_lib::scrape::extract_arxiv_id(&source_url)
                    .context("Could not extract arxiv ID for PDF fallback")?;
                let pdf_url = format!("https://arxiv.org/pdf/{arxiv_id}");
                download_and_extract_pdf(episode_id, &pdf_url, pool, config).await
            }
        };
    }

    // Article path: a single GET handles both HTML and PDF responses. Some
    // sites (e.g. nature.com, journals) serve PDFs from URLs that look like
    // article pages; others have explicit `.pdf` URLs. Either way we need to
    // route the bytes through the PDF extractor instead of running readability
    // on binary content.
    match tts_lib::scrape::fetch_article(&source_url).await? {
        tts_lib::scrape::ArticleFetch::Html { title, text } => {
            sqlx::query("UPDATE episodes SET title = $1, raw_text = $2 WHERE id = $3")
                .bind(&title)
                .bind(&text)
                .bind(episode_id)
                .execute(pool)
                .await?;
            Ok(())
        }
        tts_lib::scrape::ArticleFetch::Pdf(bytes) => {
            tracing::info!(
                "Article URL returned a PDF for {episode_id} ({source_url}); routing to PDF extraction"
            );
            let pdf_path = format!("/data/{}.pdf", episode_id);
            tokio::fs::write(&pdf_path, &bytes)
                .await
                .context("Failed to write downloaded PDF")?;
            // Upgrade source_type so the clean stage uses the academic prompt
            // (with section header extraction) instead of the article prompt.
            sqlx::query("UPDATE episodes SET source_type = 'pdf' WHERE id = $1")
                .bind(episode_id)
                .execute(pool)
                .await?;
            crate::pipeline::pdf::run(episode_id, pool, config).await
        }
    }
}

async fn save_scraped(
    episode_id: &str,
    source_url: &str,
    doc: tts_lib::Document,
    pool: &sqlx::SqlitePool,
) -> Result<()> {
    let title = doc.title.as_deref().unwrap_or(source_url);
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

async fn download_and_extract_pdf(
    episode_id: &str,
    pdf_url: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let bytes = client
        .get(pdf_url)
        .send()
        .await?
        .error_for_status()
        .context("Failed to fetch PDF")?
        .bytes()
        .await?;

    let pdf_path = format!("/data/{}.pdf", episode_id);
    tokio::fs::write(&pdf_path, &bytes)
        .await
        .context("Failed to write PDF")?;

    crate::pipeline::pdf::run(episode_id, pool, config).await
}
