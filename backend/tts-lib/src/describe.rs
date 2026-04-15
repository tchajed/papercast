use anyhow::Result;

use crate::{Document, Provider, Usage};

const SYSTEM_PROMPT: &str = r#"You are writing a short description for a podcast episode, to appear in a podcast feed.

Rules:
- 1-3 sentences, under 400 characters total.
- Summarize what the episode is about so a listener can decide whether to play it.
- Plain prose, no headings or bullets. No quoting, no leading label.
- Do not start with phrases like "This episode" or "In this paper" — just describe the content."#;

/// Generate a short episode description from a document's transcript or
/// cleaned text. Prefers transcript when present.
pub async fn describe(doc: &Document, provider: &Provider) -> Result<(String, Usage)> {
    let source = doc
        .transcript
        .as_deref()
        .or(doc.cleaned_text.as_deref())
        .ok_or_else(|| anyhow::anyhow!("No transcript or cleaned_text for description"))?;

    let snippet: String = source.chars().take(8000).collect();

    let client = reqwest::Client::new();
    let result = provider
        .chat(
            &client,
            "claude-sonnet-4-6",
            Some(SYSTEM_PROMPT),
            &snippet,
            400,
        )
        .await?;
    Ok((result.text.trim().to_string(), result.usage))
}
