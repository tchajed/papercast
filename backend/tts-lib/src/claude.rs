use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Upper bound on a single Claude API call. Prevents a hung connection from
/// blocking a worker indefinitely; the job layer will then retry with backoff.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(600);

/// Shared Claude API types and helpers used across pipeline stages.

#[derive(Serialize)]
pub struct Request {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<Vec<SystemBlock>>,
    pub messages: Vec<Message>,
}

#[derive(Serialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

#[derive(Serialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Serialize)]
pub struct Message {
    pub role: String,
    pub content: MessageContent,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ContentBlock {
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
pub struct ImageSource {
    pub r#type: String,
    pub media_type: String,
    pub data: String,
}

#[derive(Deserialize)]
pub struct Response {
    pub content: Vec<ResponseBlock>,
    #[serde(default)]
    pub usage: Option<ResponseUsage>,
}

#[derive(Deserialize, Default)]
pub struct ResponseUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum ResponseBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

impl Response {
    pub fn text(&self) -> Option<&str> {
        self.content.first().map(|ResponseBlock::Text { text }| text.as_str())
    }
}

/// Send a simple text-in/text-out request to Claude. Returns text + usage.
/// When `cache_system` is true and a system prompt is present, it is marked
/// with `cache_control: ephemeral` so repeated calls with the same system
/// prompt are billed at ~10% of input cost on cache hit.
pub async fn chat(
    client: &reqwest::Client,
    api_key: &str,
    model: &str,
    system: Option<&str>,
    user_message: &str,
    max_tokens: u32,
    cache_system: bool,
) -> Result<crate::ChatResult> {
    let system_blocks = system.map(|s| {
        vec![SystemBlock {
            block_type: "text".to_string(),
            text: s.to_string(),
            cache_control: cache_system.then(|| CacheControl {
                ty: "ephemeral".to_string(),
            }),
        }]
    });
    let request = Request {
        model: model.to_string(),
        max_tokens,
        temperature: 0.0,
        system: system_blocks,
        messages: vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(user_message.to_string()),
        }],
    };

    let input_chars = user_message.len();
    tracing::info!(
        "Claude chat start: model={model} input_chars={input_chars} max_tokens={max_tokens}"
    );
    let started = Instant::now();

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .timeout(REQUEST_TIMEOUT)
        .json(&request)
        .send()
        .await
        .with_context(|| format!("Claude request failed (model={model}, input_chars={input_chars}, elapsed={:?})", started.elapsed()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        tracing::error!(
            "Claude API error: model={model} status={status} elapsed={:?} body={body}",
            started.elapsed()
        );
        anyhow::bail!("Claude API failed ({status}): {body}");
    }

    let claude_resp: Response = resp
        .json()
        .await
        .context("Claude response JSON parse failed")?;
    let text = claude_resp
        .text()
        .map(|s| s.to_string())
        .context("Empty response from Claude")?;
    let resp_usage = claude_resp.usage.unwrap_or_default();
    tracing::info!(
        "Claude chat done: model={model} input_tokens={} output_tokens={} output_chars={} elapsed={:?}",
        resp_usage.input_tokens,
        resp_usage.output_tokens,
        text.len(),
        started.elapsed()
    );
    Ok(crate::ChatResult {
        text,
        usage: crate::Usage {
            provider: "claude".into(),
            model: model.to_string(),
            input_tokens: resp_usage.input_tokens,
            output_tokens: resp_usage.output_tokens,
        },
    })
}
