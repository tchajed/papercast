use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;

const ARTICLE_SYSTEM_PROMPT: &str = r#"You are preparing a web article for text-to-speech conversion.
Transform the provided text so it reads naturally when spoken aloud.

Rules:
- Remove any remaining navigation text, share buttons, author bios,
  newsletter signup prompts, or other non-article content.
- Fix encoding artifacts. Curly quotes and em-dashes are fine.
- Keep the article's natural structure and flow.
- Do not summarize or omit any article content.
- Do not add commentary.
- Output only the cleaned article text, nothing else."#;

const ACADEMIC_SYSTEM_PROMPT: &str = r#"You are preparing an academic paper for text-to-speech conversion.
Transform the provided text so it reads naturally when spoken aloud.

Rules:
- Remove all citation markers: [1], [23], (Smith et al., 2019), etc.
- Remove figure and table references: "as shown in Figure 3" → omit entirely.
- Rewrite inline equations as spoken English:
    \frac{a}{b} → "a over b"
    x^2 → "x squared"
    \sum_{i=1}^{n} → "the sum from i equals 1 to n of"
    For complex equations, describe what they compute rather than reading symbol-by-symbol.
- Expand abbreviations on first use if the expansion aids comprehension.
- Replace "in the next section" / "as mentioned above" with brief inline context.
- Remove LaTeX artifacts, section numbering (e.g. "3.2 Method"), footnote markers.
- Keep all substantive content — do not summarize or omit findings, methods, or discussion.
- Output only the cleaned text, nothing else."#;

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
    content: String,
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
}

#[derive(Deserialize)]
struct ClaudeContent {
    text: String,
}

pub async fn run(
    episode_id: &str,
    pool: &sqlx::SqlitePool,
    config: &AppConfig,
) -> Result<()> {
    let (raw_text, source_type) = sqlx::query_as::<_, (Option<String>, String)>(
        "SELECT raw_text, source_type FROM episodes WHERE id = $1",
    )
    .bind(episode_id)
    .fetch_one(pool)
    .await?;

    let raw_text = raw_text.context("No raw_text available for cleaning")?;

    let system_prompt = match source_type.as_str() {
        "arxiv" | "pdf" => ACADEMIC_SYSTEM_PROMPT,
        _ => ARTICLE_SYSTEM_PROMPT,
    };

    // Sonnet for articles, Opus for academic content
    let model = match source_type.as_str() {
        "arxiv" | "pdf" => "claude-opus-4-6",
        _ => "claude-sonnet-4-6",
    };

    let client = reqwest::Client::new();
    let request = ClaudeRequest {
        model: model.to_string(),
        max_tokens: 8192,
        temperature: 0.0,
        system: system_prompt.to_string(),
        messages: vec![ClaudeMessage {
            role: "user".to_string(),
            content: raw_text,
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &config.anthropic_api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?
        .error_for_status()
        .context("Claude API request failed")?;

    let claude_resp: ClaudeResponse = resp.json().await?;
    let cleaned_text = claude_resp
        .content
        .first()
        .map(|c| c.text.clone())
        .context("Empty response from Claude")?;

    let word_count = cleaned_text.split_whitespace().count() as i32;
    tracing::info!(
        "Cleaning complete for episode {episode_id}: {word_count} words"
    );

    sqlx::query("UPDATE episodes SET cleaned_text = $1, word_count = $2 WHERE id = $3")
        .bind(&cleaned_text)
        .bind(word_count)
        .bind(episode_id)
        .execute(pool)
        .await?;

    Ok(())
}
