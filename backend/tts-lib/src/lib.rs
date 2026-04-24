pub mod claude;
pub mod clean;
pub mod describe;
pub mod gemini;
pub mod image;
pub mod pdf;
pub mod pdf_gemini;
pub mod scrape;
pub mod summarize;
pub mod tts;

use serde::{Deserialize, Serialize};

/// Token usage reported by a single AI API call. Used for cost accounting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub provider: String,
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// Text response plus its usage metadata.
#[derive(Debug, Clone)]
pub struct ChatResult {
    pub text: String,
    pub usage: Usage,
}

/// AI provider for text-based tasks (clean, summarize).
#[derive(Debug, Clone)]
pub enum Provider {
    Claude { api_key: String },
    Gemini { api_key: String, model: String },
}

impl Provider {
    pub fn gemini_default(api_key: impl Into<String>) -> Self {
        Provider::Gemini {
            api_key: api_key.into(),
            model: gemini::DEFAULT_MODEL.to_string(),
        }
    }

    pub fn claude(api_key: impl Into<String>) -> Self {
        Provider::Claude {
            api_key: api_key.into(),
        }
    }

    pub async fn chat(
        &self,
        client: &reqwest::Client,
        claude_model: &str,
        system: Option<&str>,
        user_message: &str,
        max_output_tokens: u32,
    ) -> anyhow::Result<ChatResult> {
        self.chat_opts(
            client,
            claude_model,
            system,
            user_message,
            max_output_tokens,
            false,
        )
        .await
    }

    /// Like `chat`, but with opt-in system-prompt caching (Claude only; Gemini
    /// ignores the flag). Use when the same system prompt is sent repeatedly.
    pub async fn chat_opts(
        &self,
        client: &reqwest::Client,
        claude_model: &str,
        system: Option<&str>,
        user_message: &str,
        max_output_tokens: u32,
        cache_system: bool,
    ) -> anyhow::Result<ChatResult> {
        match self {
            Provider::Claude { api_key } => {
                claude::chat(
                    client,
                    api_key,
                    claude_model,
                    system,
                    user_message,
                    max_output_tokens,
                    cache_system,
                )
                .await
            }
            Provider::Gemini { api_key, model } => {
                gemini::chat(
                    client,
                    api_key,
                    model,
                    system,
                    user_message,
                    max_output_tokens,
                )
                .await
            }
        }
    }
}

/// Shared document representation that flows through pipeline stages.
/// Each stage reads the fields it needs and populates its output fields.
/// This is the JSON format used by the CLI between stages.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Document {
    /// Document title (set by extract/scrape, may be updated by pdf extraction)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Source type: "article", "arxiv", or "pdf"
    #[serde(default = "default_source_type")]
    pub source_type: String,

    /// Raw extracted text (from scrape or PDF extraction)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_text: Option<String>,

    /// Cleaned text ready for TTS
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cleaned_text: Option<String>,

    /// Summarized transcript (optional, replaces cleaned_text for TTS)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript: Option<String>,

    /// Word count of the final text
    #[serde(skip_serializing_if = "Option::is_none")]
    pub word_count: Option<usize>,
}

fn default_source_type() -> String {
    "pdf".to_string()
}

/// Text that should be sent to TTS — transcript if available, otherwise cleaned_text.
impl Document {
    pub fn tts_text(&self) -> Option<&str> {
        self.transcript.as_deref().or(self.cleaned_text.as_deref())
    }
}
