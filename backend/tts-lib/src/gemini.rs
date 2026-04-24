use anyhow::{Context, Result};
use serde_json::json;
use std::time::{Duration, Instant};

/// Default Gemini model for text tasks. `flash-latest` is the stable alias
/// that points to the current flash model and is less prone to 503s than
/// a pinned minor version.
pub const DEFAULT_MODEL: &str = "gemini-flash-latest";

/// Upper bound on a single Gemini API call. Prevents a hung connection from
/// blocking a worker indefinitely; the job layer will then retry with backoff.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Send a text-in / text-out request to Gemini.
pub async fn chat(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    user_message: &str,
    max_output_tokens: u32,
) -> Result<crate::ChatResult> {
    let mut body = json!({
        "contents": [{
            "parts": [{ "text": user_message }]
        }],
        "generationConfig": {
            "temperature": 0.0,
            "maxOutputTokens": max_output_tokens,
        }
    });

    if let Some(sys) = system {
        body["systemInstruction"] = json!({
            "parts": [{ "text": sys }]
        });
    }

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, api_key
    );

    let input_chars = user_message.len();
    tracing::info!(
        "Gemini chat start: model={model} input_chars={input_chars} max_tokens={max_output_tokens}"
    );
    let started = Instant::now();

    let resp = client
        .post(&url)
        .timeout(REQUEST_TIMEOUT)
        .json(&body)
        .send()
        .await
        .with_context(|| {
            format!(
                "Gemini request failed (model={model}, input_chars={input_chars}, elapsed={:?})",
                started.elapsed()
            )
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(
            "Gemini API error: model={model} status={status} elapsed={:?} body={body}",
            started.elapsed()
        );
        anyhow::bail!("Gemini API failed ({status}): {body}");
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .context("Gemini response JSON parse failed")?;
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .context("No parts in Gemini response")?;

    let text: String = parts
        .iter()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("");

    if text.is_empty() {
        let finish_reason = body["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("unknown");
        tracing::error!(
            "Gemini empty response: model={model} finish_reason={finish_reason} elapsed={:?}",
            started.elapsed()
        );
        anyhow::bail!("Empty response from Gemini (finishReason={finish_reason})");
    }

    let usage_meta = &body["usageMetadata"];
    let input_tokens = usage_meta["promptTokenCount"].as_u64().unwrap_or(0) as u32;
    let output_tokens = usage_meta["candidatesTokenCount"].as_u64().unwrap_or(0) as u32;
    tracing::info!(
        "Gemini chat done: model={model} input_tokens={input_tokens} output_tokens={output_tokens} output_chars={} elapsed={:?}",
        text.len(),
        started.elapsed()
    );
    Ok(crate::ChatResult {
        text,
        usage: crate::Usage {
            provider: "gemini".into(),
            model: model.to_string(),
            input_tokens,
            output_tokens,
        },
    })
}
