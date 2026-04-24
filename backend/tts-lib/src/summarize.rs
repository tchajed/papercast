use anyhow::Result;

use crate::{Document, Provider, Usage};

const SUMMARIZE_SYSTEM_PROMPT: &str = r#"You are preparing a concise podcast-style summary of a text.
Condense the content into a clear, engaging summary suitable for listening.

Rules:
- Capture the key ideas, findings, and arguments.
- Aim for roughly 20-30% of the original length.
- Use natural, spoken-style language — this will be read aloud by TTS.
- Maintain the logical flow: introduce the topic, cover main points, conclude.
- Do not add your own opinions or commentary.
- Do not use bullet points or numbered lists — write in flowing paragraphs.
- Output only the summary text, nothing else."#;

/// Summarize cleaned text into a podcast-style transcript. An optional `focus`
/// narrows the summary to a particular topic / angle.
pub async fn summarize(
    doc: &Document,
    provider: &Provider,
    focus: Option<&str>,
) -> Result<(Document, Usage)> {
    let cleaned_text = doc
        .cleaned_text
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No cleaned_text available for summarization"))?;

    let system_prompt = match focus.map(str::trim).filter(|s| !s.is_empty()) {
        Some(f) => format!(
            "{SUMMARIZE_SYSTEM_PROMPT}\n\nFocus: {f}\nPrioritize content related to this focus and omit or compress everything else. If the focus is entirely absent from the source, summarize the source normally."
        ),
        None => SUMMARIZE_SYSTEM_PROMPT.to_string(),
    };

    let client = reqwest::Client::new();
    let result = provider
        .chat(
            &client,
            "claude-sonnet-4-6",
            Some(&system_prompt),
            cleaned_text,
            8192,
        )
        .await?;
    let transcript = result.text;

    let word_count = transcript.split_whitespace().count();
    tracing::info!("Summarization complete: {word_count} words");

    Ok((
        Document {
            transcript: Some(transcript),
            word_count: Some(word_count),
            ..doc.clone()
        },
        result.usage,
    ))
}
