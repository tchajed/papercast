use anyhow::{Context, Result};
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

/// PDF extraction via page-by-page image rendering + Claude vision.
/// Uses pdftoppm to render each page as a JPEG, then sends each page
/// to Claude for text extraction and concatenates the results.
pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let pdf_path = format!("/tmp/{}.pdf", episode_id);
    let page_dir = format!("/tmp/{}_pages", episode_id);

    // Create output directory for page images
    tokio::fs::create_dir_all(&page_dir).await?;

    // Render PDF pages to JPEG using pdftoppm
    let output = tokio::process::Command::new("pdftoppm")
        .args(["-jpeg", "-r", "200", &pdf_path, &format!("{}/page", page_dir)])
        .output()
        .await
        .context("Failed to run pdftoppm")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pdftoppm failed: {stderr}");
    }

    // Collect page image files in order
    let mut page_files: Vec<String> = Vec::new();
    let mut entries = tokio::fs::read_dir(&page_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "jpg") {
            page_files.push(path.to_string_lossy().to_string());
        }
    }
    page_files.sort();

    if page_files.is_empty() {
        anyhow::bail!("No pages rendered from PDF");
    }

    tracing::info!(
        "PDF rendered {} pages for episode {episode_id}",
        page_files.len()
    );

    // Process each page through Claude vision
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;
    let mut all_text = Vec::new();

    for (i, page_path) in page_files.iter().enumerate() {
        let page_bytes = tokio::fs::read(page_path).await?;
        let page_b64 = base64::engine::general_purpose::STANDARD.encode(&page_bytes);

        tracing::info!(
            "Extracting text from page {}/{} for episode {episode_id}",
            i + 1,
            page_files.len()
        );

        let request = ClaudeRequest {
            model: "claude-sonnet-4-6".to_string(),
            max_tokens: 4096,
            temperature: 0.0,
            system: PDF_SYSTEM_PROMPT.to_string(),
            messages: vec![ClaudeMessage {
                role: "user".to_string(),
                content: vec![
                    ContentBlock::Image {
                        r#type: "image".to_string(),
                        source: ImageSource {
                            r#type: "base64".to_string(),
                            media_type: "image/jpeg".to_string(),
                            data: page_b64,
                        },
                    },
                    ContentBlock::Text {
                        r#type: "text".to_string(),
                        text: format!("Extract all text from page {} of this document.", i + 1),
                    },
                ],
            }],
        };

        let resp = client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &config.anthropic_api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            // Skip pages blocked by content filtering, bail on other errors
            if body.contains("content filtering") {
                tracing::warn!("Page {} blocked by content filter, skipping", i + 1);
                continue;
            }
            tracing::error!("Claude API error on page {}: {status} {body}", i + 1);
            anyhow::bail!("Claude API failed on page {} ({status}): {body}", i + 1);
        }

        let claude_resp: ClaudeResponse = resp.json().await?;
        let page_text: String = claude_resp
            .content
            .iter()
            .map(|ResponseBlock::Text { text }| text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        if !page_text.is_empty() {
            all_text.push(page_text);
        }
    }

    let raw_text = all_text.join("\n\n");

    if raw_text.is_empty() {
        anyhow::bail!("Empty text extracted from PDF");
    }

    // Extract title if episode title is still "PDF Upload"
    let current_title =
        sqlx::query_scalar::<_, String>("SELECT title FROM episodes WHERE id = $1")
            .bind(episode_id)
            .fetch_one(pool)
            .await?;

    let title = if current_title == "PDF Upload" {
        extract_title_from_text(&client, config, &raw_text).await?
    } else {
        current_title
    };

    sqlx::query("UPDATE episodes SET title = $1, raw_text = $2 WHERE id = $3")
        .bind(&title)
        .bind(&raw_text)
        .bind(episode_id)
        .execute(pool)
        .await?;

    // Clean up temp files
    let _ = tokio::fs::remove_file(&pdf_path).await;
    let _ = tokio::fs::remove_dir_all(&page_dir).await;

    Ok(())
}

async fn extract_title_from_text(
    client: &reqwest::Client,
    config: &AppConfig,
    text: &str,
) -> Result<String> {
    let snippet: String = text.chars().take(2000).collect();

    let request = serde_json::json!({
        "model": "claude-sonnet-4-6",
        "max_tokens": 100,
        "temperature": 0.0,
        "messages": [{
            "role": "user",
            "content": format!(
                "What is the title of this document? Output only the title, nothing else.\n\n{}",
                snippet
            ),
        }],
    });

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &config.anthropic_api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?
        .error_for_status()?;

    let claude_resp: ClaudeResponse = resp.json().await?;
    let title = claude_resp
        .content
        .first()
        .map(|ResponseBlock::Text { text }| text.clone())
        .unwrap_or_else(|| "Untitled PDF".into());

    Ok(title.trim().to_string())
}

const PDF_SYSTEM_PROMPT: &str = r#"You are extracting text from a page of a PDF document for text-to-speech conversion.

Rules:
- Extract all text content in reading order (top-to-bottom, respecting column layout).
- For two-column layouts, complete the left column before the right column.
- Ignore page numbers, headers, footers, and running titles.
- Ignore figure captions and table captions — replace with "[Figure omitted]" or "[Table omitted]".
- Preserve paragraph breaks.
- Output only the extracted text, nothing else."#;

#[derive(Serialize)]
struct ClaudeRequest {
    model: String,
    max_tokens: u32,
    temperature: f32,
    system: String,
    messages: Vec<ClaudeMessage>,
}

#[derive(Serialize)]
struct ClaudeMessage {
    role: String,
    content: Vec<ContentBlock>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum ContentBlock {
    Image {
        r#type: String,
        source: ImageSource,
    },
    Text {
        r#type: String,
        text: String,
    },
}

#[derive(Serialize)]
struct ImageSource {
    r#type: String,
    media_type: String,
    data: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ResponseBlock>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
}
