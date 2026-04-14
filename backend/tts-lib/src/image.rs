use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;

use crate::Provider;

const VISUAL_SUMMARY_PROMPT: &str = "Summarize this content in exactly two sentences suitable for a visual illustration prompt. Focus on the core subject matter. Output only the two sentences, nothing else.";

/// Image-generating Gemini model ("Nano Banana"). Plain gemini-2.5-flash
/// does not return image parts.
pub const DEFAULT_IMAGE_MODEL: &str = "gemini-2.5-flash-image";

pub struct GeneratedImage {
    pub bytes: Bytes,
    pub mime_type: String,
}

/// Produce a two-sentence visual brief for a cover-image prompt.
pub async fn visual_summary(text: &str, provider: &Provider) -> Result<String> {
    let snippet: String = text.chars().take(4000).collect();
    let client = reqwest::Client::new();
    let summary = provider
        .chat(
            &client,
            "claude-sonnet-4-6",
            None,
            &format!("{VISUAL_SUMMARY_PROMPT}\n\n{snippet}"),
            200,
        )
        .await?;
    Ok(summary.trim().to_string())
}

/// Generate a cover image from a short visual-brief summary.
pub async fn generate_image(
    google_api_key: &str,
    summary: &str,
) -> Result<GeneratedImage> {
    generate_image_with_model(google_api_key, summary, DEFAULT_IMAGE_MODEL).await
}

pub async fn generate_image_with_model(
    google_api_key: &str,
    summary: &str,
    model: &str,
) -> Result<GeneratedImage> {
    let prompt = format!(
        "Create a simple, clean illustration for a podcast episode about: {summary}. Minimal style, bold shapes, suitable as a podcast episode thumbnail at small sizes. No text or labels in the image."
    );

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={google_api_key}"
    );

    let request = serde_json::json!({
        "contents": [{ "parts": [{ "text": prompt }] }],
        "generationConfig": { "responseModalities": ["IMAGE"] }
    });

    let client = reqwest::Client::new();
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
