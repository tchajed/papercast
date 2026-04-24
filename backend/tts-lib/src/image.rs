use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;

use crate::{Provider, Usage};

const VISUAL_SUMMARY_PROMPT: &str = "Summarize this content in exactly two sentences suitable for a visual illustration prompt. Focus on the core subject matter. Output only the two sentences, nothing else.";

/// Image-generating Gemini model ("Nano Banana"). Plain gemini-2.5-flash
/// does not return image parts.
pub const DEFAULT_IMAGE_MODEL: &str = "gemini-2.5-flash-image";

pub struct GeneratedImage {
    pub bytes: Bytes,
    pub mime_type: String,
}

/// Produce a two-sentence visual brief for a cover-image prompt.
pub async fn visual_summary(text: &str, provider: &Provider) -> Result<(String, Usage)> {
    let snippet: String = text.chars().take(4000).collect();
    let client = reqwest::Client::new();
    let result = provider
        .chat(
            &client,
            "claude-sonnet-4-6",
            None,
            &format!("{VISUAL_SUMMARY_PROMPT}\n\n{snippet}"),
            200,
        )
        .await?;
    Ok((result.text.trim().to_string(), result.usage))
}

/// Generate a cover image for a single podcast *episode* from a short
/// visual-brief summary.
pub async fn generate_image(
    google_api_key: &str,
    summary: &str,
) -> Result<(GeneratedImage, Usage)> {
    let prompt = format!(
        "Create a simple, clean illustration for a podcast episode about: {summary}. \
         Minimal style, bold shapes, suitable as a podcast episode thumbnail at small sizes. \
         No text or labels in the image. \
         The image MUST be square (1:1 aspect ratio) with the subject centered — \
         podcast artwork is displayed as a square, so keep all important content \
         well inside the central square region."
    );
    generate_from_prompt(google_api_key, &prompt, DEFAULT_IMAGE_MODEL).await
}

/// Generate a cover image for a podcast *feed* (channel-level artwork).
/// Channel covers are the first thing a listener sees in their library and
/// persist across every episode, so they get a more considered prompt than
/// per-episode thumbnails: stronger visual identity, richer composition,
/// and explicit emphasis on legibility at small sizes.
pub async fn generate_feed_cover(
    google_api_key: &str,
    brief: &str,
) -> Result<(GeneratedImage, Usage)> {
    let prompt = format!(
        "Design a distinctive cover image for a podcast feed titled and described as: {brief}.\n\n\
         This is the channel-level artwork that listeners will see whenever they browse \
         their podcast library, so it must have strong visual identity and read clearly at \
         small sizes (down to ~55x55 pixels in a list view).\n\n\
         Requirements:\n\
         - Square (1:1 aspect ratio) with the subject well-centered. All important \
           elements must sit inside the middle 80% of the square; nothing important near \
           the edges. Podcast clients display this as a square and may crop further.\n\
         - A single clear focal subject or symbol that represents the feed's theme. \
           Avoid busy collages or multiple competing subjects.\n\
         - Bold shapes, strong silhouette, high contrast between subject and background. \
           Avoid thin lines and fine detail that will vanish when scaled down.\n\
         - A cohesive, limited color palette (2-4 colors) that feels intentional and \
           memorable.\n\
         - Flat or lightly textured illustration style. Avoid photorealism, clutter, and \
           gradients that muddy the image at small sizes.\n\
         - Absolutely no text, letters, numbers, or logos anywhere in the image."
    );
    generate_from_prompt(google_api_key, &prompt, DEFAULT_IMAGE_MODEL).await
}

async fn generate_from_prompt(
    google_api_key: &str,
    prompt: &str,
    model: &str,
) -> Result<(GeneratedImage, Usage)> {
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

    // Force a square aspect ratio: Gemini doesn't always honor the prompt and
    // podcast clients (Overcast) silently reject non-square cover art.
    let (cropped_bytes, final_mime) = center_crop_square(&image_bytes, &mime_type)?;

    let usage_meta = &body["usageMetadata"];
    let input_tokens = usage_meta["promptTokenCount"].as_u64().unwrap_or(0) as u32;
    let output_tokens = usage_meta["candidatesTokenCount"].as_u64().unwrap_or(0) as u32;

    Ok((
        GeneratedImage {
            bytes: Bytes::from(cropped_bytes),
            mime_type: final_mime,
        },
        Usage {
            provider: "gemini".into(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
        },
    ))
}

/// Decode, center-crop to a square, and re-encode. If the image is already
/// square we skip the re-encode to avoid needless quality loss.
fn center_crop_square(bytes: &[u8], mime_type: &str) -> Result<(Vec<u8>, String)> {
    let mut img = image::load_from_memory(bytes).context("Failed to decode generated image")?;
    let (w, h) = (img.width(), img.height());
    if w == h {
        return Ok((bytes.to_vec(), mime_type.to_string()));
    }
    let side = w.min(h);
    let x = (w - side) / 2;
    let y = (h - side) / 2;
    let square = img.crop(x, y, side, side);

    // Always emit PNG after a crop — it's lossless and matches the common case.
    let mut out = std::io::Cursor::new(Vec::new());
    square
        .write_to(&mut out, image::ImageFormat::Png)
        .context("Failed to encode cropped image")?;
    tracing::info!("Center-cropped cover image from {w}x{h} to {side}x{side}");
    Ok((out.into_inner(), "image/png".to_string()))
}
