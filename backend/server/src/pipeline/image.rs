use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;

use crate::config::AppConfig;
use crate::pipeline::storage::StorageClient;

/// Generate a cover image for an episode using Gemini.
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

    let google_api_key = &config.google_studio_api_key;

    let cleaned_text = sqlx::query_scalar::<_, Option<String>>(
        "SELECT cleaned_text FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?
    .context("No cleaned_text for image generation")?;

    let client = reqwest::Client::new();

    // Step 1: Generate a two-sentence summary via Claude
    let summary = generate_summary(&client, config, &cleaned_text).await?;

    // Step 2: Generate image via Gemini
    let image = generate_image(&client, google_api_key, &summary).await?;

    // Step 3: Upload to Tigris
    let image_url = storage
        .upload_episode_image(episode_id, image.bytes, &image.mime_type)
        .await?;

    // Step 4: Patch episode
    sqlx::query("UPDATE episodes SET image_url = $1 WHERE id = $2")
        .bind(&image_url)
        .bind(episode_id)
        .execute(pool)
        .await?;

    tracing::info!("Generated cover image for episode {episode_id}");
    Ok(())
}

async fn generate_summary(
    client: &reqwest::Client,
    config: &AppConfig,
    text: &str,
) -> Result<String> {
    // Use first ~4000 chars for summary to save tokens
    let snippet: String = text.chars().take(4000).collect();

    let request = serde_json::json!({
        "model": "claude-sonnet-4-6",
        "max_tokens": 200,
        "temperature": 0.0,
        "messages": [{
            "role": "user",
            "content": format!(
                "Summarize this content in exactly two sentences suitable for a visual illustration prompt. Focus on the core subject matter. Output only the two sentences, nothing else.\n\n{}",
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

    #[derive(serde::Deserialize)]
    struct Resp {
        content: Vec<Content>,
    }
    #[derive(serde::Deserialize)]
    struct Content {
        text: String,
    }

    let r: Resp = resp.json().await?;
    Ok(r.content
        .first()
        .map(|c| c.text.clone())
        .unwrap_or_default())
}

pub struct GeneratedImage {
    pub bytes: Bytes,
    pub mime_type: String,
}

async fn generate_image(
    client: &reqwest::Client,
    api_key: &str,
    summary: &str,
) -> Result<GeneratedImage> {
    let prompt = format!(
        "Create a simple, clean illustration for a podcast episode about: {}. Minimal style, bold shapes, suitable as a podcast episode thumbnail at small sizes. No text or labels in the image.",
        summary
    );

    // gemini-2.5-flash-image (aka "Nano Banana") is the image-generating
    // variant; plain gemini-2.5-flash does not return image parts.
    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-image:generateContent?key={}",
        api_key
    );

    let request = serde_json::json!({
        "contents": [{
            "parts": [{ "text": prompt }]
        }],
        "generationConfig": {
            "responseModalities": ["IMAGE"]
        }
    });

    let resp = client.post(&url).json(&request).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Gemini image generation failed ({status}): {body}");
    }
    let body: serde_json::Value = resp.json().await?;

    let inline = body["candidates"][0]["content"]["parts"]
        .as_array()
        .and_then(|parts| parts.iter().find(|p| p.get("inlineData").is_some()))
        .and_then(|p| p.get("inlineData"))
        .context("No inlineData image part in Gemini response")?;

    let image_b64 = inline["data"]
        .as_str()
        .context("No image data in Gemini response")?;
    let mime_type = inline["mimeType"]
        .as_str()
        .unwrap_or("image/png")
        .to_string();

    let image_bytes = base64::engine::general_purpose::STANDARD
        .decode(image_b64)
        .context("Failed to decode Gemini image")?;

    Ok(GeneratedImage {
        bytes: Bytes::from(image_bytes),
        mime_type,
    })
}
