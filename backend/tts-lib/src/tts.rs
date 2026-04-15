use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const TTS_CONCURRENCY: usize = 4;
const MAX_CHUNK_CHARS: usize = 4000;

/// 1.5s of silence (24 kHz mono MP3) appended between sections. Used instead
/// of an SSML `<break>` because Journey voices (the default in production)
/// reject SSML input entirely.
const SECTION_SILENCE_MP3: &[u8] = include_bytes!("silence_1500ms.mp3");

/// TTS configuration.
pub struct TtsConfig {
    pub google_api_key: String,
    pub voice: String,
}

impl TtsConfig {
    pub fn new(google_api_key: String) -> Self {
        Self {
            google_api_key,
            voice: "en-US-Journey-D".to_string(),
        }
    }

    pub fn with_voice(mut self, voice: String) -> Self {
        self.voice = voice;
        self
    }
}

/// A section in the final audio, keyed by its start time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    pub start_secs: f64,
}

/// Result of TTS synthesis.
pub struct TtsResult {
    pub audio: Bytes,
    pub duration_secs: u32,
    pub chunks_total: usize,
    /// Section timestamps, or empty if the input had no `## ` headers.
    pub sections: Vec<Section>,
}

/// Progress callback called after each chunk completes.
pub type ProgressCallback = Arc<dyn Fn(usize, usize) + Send + Sync>;

/// Internal chunk descriptor for TTS synthesis.
struct Chunk {
    text: String,
    section_idx: usize,
    /// True if this is the final chunk of its section AND not the last section
    /// overall — a silent MP3 is appended after synthesis.
    append_pause: bool,
}

/// Synthesize text to MP3 audio using Google Cloud TTS.
///
/// Recognizes `## Section Title` markdown headers at the start of a line to
/// split the input into sections. Each section boundary gets a 1.5s pause,
/// and the returned `sections` vec carries the start time of each section
/// within the final audio. If no headers are present, `sections` is empty.
pub async fn synthesize(
    text: &str,
    config: &TtsConfig,
    on_progress: Option<ProgressCallback>,
) -> Result<TtsResult> {
    let sections_text = parse_sections(text);
    let has_sections = !sections_text.is_empty() && sections_text.iter().any(|s| s.title.is_some());
    let sections_for_chunking = if sections_text.is_empty() {
        vec![SectionText { title: None, body: text.to_string() }]
    } else {
        sections_text
    };

    let chunks = build_chunks(&sections_for_chunking, MAX_CHUNK_CHARS);
    let total_chunks = chunks.len();
    let word_count = text.split_whitespace().count();
    // Chunk metadata needed after the stream consumes `chunks`.
    let chunk_section_idxs: Vec<usize> = chunks.iter().map(|c| c.section_idx).collect();
    let chunk_word_counts: Vec<usize> =
        chunks.iter().map(|c| c.text.split_whitespace().count()).collect();

    tracing::info!(
        "TTS starting: {word_count} words, {total_chunks} chunks across {} sections (~{:.0}s estimated)",
        sections_for_chunking.len(),
        word_count as f64 * 0.13
    );

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()?;

    let completed = Arc::new(AtomicUsize::new(0));
    let on_progress = on_progress.clone();

    let results: Vec<Result<(usize, Bytes)>> = stream::iter(chunks.into_iter().enumerate())
        .map(|(i, chunk)| {
            let client = client.clone();
            let completed = completed.clone();
            let on_progress = on_progress.clone();
            async move {
                let chunk_words = chunk.text.split_whitespace().count();
                tracing::info!("TTS chunk {}/{} ({chunk_words} words)", i + 1, total_chunks);
                let mut audio = tts_google(&client, config, &chunk.text).await?;
                if chunk.append_pause {
                    // Append a 1.5s silent MP3 segment so the chunk's measured
                    // duration (and the running audio offset) includes the pause.
                    let mut combined =
                        Vec::with_capacity(audio.len() + SECTION_SILENCE_MP3.len());
                    combined.extend_from_slice(&audio);
                    combined.extend_from_slice(SECTION_SILENCE_MP3);
                    audio = Bytes::from(combined);
                }
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(ref cb) = on_progress {
                    cb(done, total_chunks);
                }
                Ok::<_, anyhow::Error>((i, audio))
            }
        })
        .buffer_unordered(TTS_CONCURRENCY)
        .collect()
        .await;

    let mut indexed: Vec<(usize, Bytes)> = results.into_iter().collect::<Result<_>>()?;
    indexed.sort_by_key(|(i, _)| *i);
    let audio_parts: Vec<Bytes> = indexed.into_iter().map(|(_, b)| b).collect();

    // Per-chunk durations; fall back to word-rate estimate if MP3 parsing fails
    // for a chunk so section offsets remain approximately correct.
    let per_chunk_durations: Vec<f64> = audio_parts
        .iter()
        .zip(chunk_word_counts.iter())
        .map(|(bytes, &words)| {
            mp3_duration::from_read(&mut std::io::Cursor::new(&bytes[..]))
                .map(|d| d.as_secs_f64())
                .unwrap_or_else(|_| words as f64 / 150.0 * 60.0)
        })
        .collect();

    // Compute section start times by summing chunk durations before each
    // section's first chunk.
    let sections = if has_sections {
        build_section_timeline(&sections_for_chunking, &chunk_section_idxs, &per_chunk_durations)
    } else {
        Vec::new()
    };

    // Concatenate MP3 chunks
    let total_bytes: Vec<u8> = audio_parts.iter().flat_map(|b| b.to_vec()).collect();
    let audio = Bytes::from(total_bytes);

    let duration_secs = mp3_duration::from_read(&mut std::io::Cursor::new(&audio[..]))
        .map(|d| d.as_secs() as u32)
        .unwrap_or_else(|_| per_chunk_durations.iter().sum::<f64>() as u32);

    Ok(TtsResult {
        audio,
        duration_secs,
        chunks_total: total_chunks,
        sections,
    })
}

#[derive(Debug, Clone)]
struct SectionText {
    /// None means "preface" text before the first `## ` header.
    title: Option<String>,
    body: String,
}

/// Split text on `## Section Title` markdown headers (at the start of a line).
/// Returns sections in order. Leading text before the first header becomes a
/// section with `title: None`. Returns empty if text has no `## ` headers.
fn parse_sections(text: &str) -> Vec<SectionText> {
    // Find line-starts that begin with "## " (not "### " etc.). Tolerate
    // leading whitespace since some cleaners indent header lines.
    let mut headers: Vec<(usize, String)> = Vec::new();
    for (line_start, line) in line_offsets(text) {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("## ") {
            if !rest.starts_with('#') {
                headers.push((line_start, rest.trim().to_string()));
            }
        }
    }
    tracing::info!("parse_sections: detected {} sections", headers.len());

    if headers.is_empty() {
        return Vec::new();
    }

    let mut sections = Vec::new();
    if headers[0].0 > 0 {
        let preface = text[..headers[0].0].trim();
        if !preface.is_empty() {
            sections.push(SectionText {
                title: None,
                body: preface.to_string(),
            });
        }
    }
    for i in 0..headers.len() {
        let (start, ref title) = headers[i];
        // Body starts after the header line
        let after_header = text[start..]
            .find('\n')
            .map(|n| start + n + 1)
            .unwrap_or(text.len());
        let end = headers.get(i + 1).map(|(s, _)| *s).unwrap_or(text.len());
        let body = text[after_header..end].trim().to_string();
        sections.push(SectionText {
            title: Some(title.clone()),
            body,
        });
    }
    sections
}

fn line_offsets(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut pos = 0;
    for line in text.split('\n') {
        out.push((pos, line));
        pos += line.len() + 1;
    }
    out
}

fn build_chunks(sections: &[SectionText], max_chars: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let last_section_idx = sections.len().saturating_sub(1);
    for (section_idx, section) in sections.iter().enumerate() {
        if section.body.trim().is_empty() {
            continue;
        }
        let sub = sub_chunk(&section.body, max_chars);
        let n = sub.len();
        for (i, text) in sub.into_iter().enumerate() {
            chunks.push(Chunk {
                text,
                section_idx,
                append_pause: i == n - 1 && section_idx < last_section_idx,
            });
        }
    }
    if chunks.is_empty() {
        // Nothing parsed — fall back to full text as one chunk
        let fallback: String = sections.iter().map(|s| s.body.as_str()).collect::<Vec<_>>().join("\n\n");
        chunks.push(Chunk {
            text: fallback,
            section_idx: 0,
            append_pause: false,
        });
    }
    chunks
}

fn build_section_timeline(
    sections: &[SectionText],
    chunk_section_idxs: &[usize],
    durations: &[f64],
) -> Vec<Section> {
    let mut out = Vec::new();
    let mut cumulative = 0.0_f64;
    let mut current_section: Option<usize> = None;
    for (i, &section_idx) in chunk_section_idxs.iter().enumerate() {
        if current_section != Some(section_idx) {
            current_section = Some(section_idx);
            let title = sections[section_idx]
                .title
                .clone()
                .unwrap_or_else(|| "Introduction".to_string());
            out.push(Section {
                title,
                start_secs: cumulative,
            });
        }
        cumulative += durations[i];
    }
    out
}

fn sub_chunk(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();

    for sentence in split_sentences(text) {
        if current.len() + sentence.len() > max_chars && !current.is_empty() {
            chunks.push(current.clone());
            current.clear();
        }
        current.push_str(&sentence);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    if chunks.is_empty() {
        chunks.push(text.to_string());
    }
    chunks
}

fn split_sentences(text: &str) -> Vec<String> {
    let mut sentences = Vec::new();
    let mut current = String::new();

    for ch in text.chars() {
        current.push(ch);
        if (ch == '.' || ch == '!' || ch == '?') && current.len() > 1 {
            sentences.push(current.clone());
            current.clear();
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }
    sentences
}

async fn tts_google(
    client: &reqwest::Client,
    config: &TtsConfig,
    text: &str,
) -> Result<Bytes> {
    let url = format!(
        "https://texttospeech.googleapis.com/v1/text:synthesize?key={}",
        config.google_api_key
    );

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "input": { "text": text },
            "voice": {
                "languageCode": "en-US",
                "name": config.voice,
            },
            "audioConfig": { "audioEncoding": "MP3" },
        }))
        .send()
        .await
        .context("Google TTS request send failed")?;

    let status = resp.status();
    if !status.is_success() {
        // Capture the response body so the error message identifies the actual
        // failure (quota exceeded, invalid voice, auth, etc.) rather than just
        // "request failed". Truncate defensively to avoid enormous error rows.
        let body = resp.text().await.unwrap_or_default();
        let truncated: String = body.chars().take(1000).collect();
        anyhow::bail!("Google TTS {status}: {truncated}");
    }

    let body: serde_json::Value = resp.json().await.context("Google TTS response parse failed")?;
    let audio_b64 = body["audioContent"]
        .as_str()
        .context("No audioContent in Google TTS response")?;

    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .context("Failed to decode Google TTS audio")?;

    Ok(Bytes::from(audio_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_sentences_basic() {
        let result = split_sentences("Hello world. How are you? Fine!");
        assert_eq!(result, vec!["Hello world.", " How are you?", " Fine!"]);
    }

    #[test]
    fn test_split_sentences_no_punctuation() {
        let result = split_sentences("No ending punctuation");
        assert_eq!(result, vec!["No ending punctuation"]);
    }

    #[test]
    fn test_sub_chunk_single() {
        let chunks = sub_chunk("Short text.", 100);
        assert_eq!(chunks, vec!["Short text."]);
    }

    #[test]
    fn test_sub_chunk_splits_on_sentence_boundary() {
        let chunks = sub_chunk("First sentence. Second sentence. Third sentence.", 20);
        assert_eq!(chunks.len(), 3);
    }

    #[test]
    fn test_parse_sections_none() {
        let r = parse_sections("Just a body with no headers.\n\nMore text.");
        assert!(r.is_empty());
    }

    #[test]
    fn test_parse_sections_basic() {
        let text = "## Abstract\n\nAbstract body.\n\n## Introduction\n\nIntro body.";
        let r = parse_sections(text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].title.as_deref(), Some("Abstract"));
        assert_eq!(r[0].body, "Abstract body.");
        assert_eq!(r[1].title.as_deref(), Some("Introduction"));
        assert_eq!(r[1].body, "Intro body.");
    }

    #[test]
    fn test_parse_sections_with_preface() {
        let text = "Preamble.\n\n## Section One\n\nBody.";
        let r = parse_sections(text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].title, None);
        assert_eq!(r[0].body, "Preamble.");
        assert_eq!(r[1].title.as_deref(), Some("Section One"));
    }

    #[test]
    fn test_parse_sections_ignores_subheaders() {
        let text = "## Main\n\nBody.\n\n### Sub\n\nSubbody.";
        let r = parse_sections(text);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title.as_deref(), Some("Main"));
        assert!(r[0].body.contains("### Sub"));
    }

    #[test]
    fn test_build_chunks_marks_section_ends() {
        let sections = vec![
            SectionText { title: Some("A".into()), body: "Sentence one.".into() },
            SectionText { title: Some("B".into()), body: "Sentence two.".into() },
        ];
        let chunks = build_chunks(&sections, 1000);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].append_pause, "end of non-final section should pause");
        assert!(!chunks[1].append_pause, "final section does not pause");
    }

    #[test]
    fn test_build_section_timeline() {
        let sections = vec![
            SectionText { title: Some("A".into()), body: "x".into() },
            SectionText { title: Some("B".into()), body: "y".into() },
        ];
        let idxs = vec![0, 1];
        let durations = vec![10.0, 5.0];
        let tl = build_section_timeline(&sections, &idxs, &durations);
        assert_eq!(tl.len(), 2);
        assert_eq!(tl[0].title, "A");
        assert_eq!(tl[0].start_secs, 0.0);
        assert_eq!(tl[1].title, "B");
        assert_eq!(tl[1].start_secs, 10.0);
    }

}
