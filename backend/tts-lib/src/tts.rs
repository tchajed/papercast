use anyhow::{Context, Result};
use base64::Engine;
use bytes::Bytes;
use futures::stream::{self, StreamExt};
use id3::frame::{Chapter, Frame, TableOfContents};
use id3::{Tag, TagLike, Version};
use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const TTS_CONCURRENCY: usize = 4;
/// Budget for the XML-escaped SSML payload sent to Google (5000-byte hard
/// limit). Leaves headroom for `<speak>/<p>/<s>` tags, escape expansion
/// (`&` → `&amp;`), and non-ASCII bytes.
const MAX_CHUNK_CHARS: usize = 3600;
/// Google TTS rejects individual sentences longer than ~900 chars with
/// "This request contains sentences that are too long." Anything beyond
/// this is split at secondary punctuation (see `split_long_sentence`).
const MAX_SENTENCE_CHARS: usize = 900;

/// 1.5s of silence (24 kHz mono MP3) concatenated between section chunks.
/// Each chunk is a separate Google TTS request, so a single SSML `<break>`
/// can't span them — the silent MP3 bridges the gap.
const SECTION_SILENCE_MP3: &[u8] = include_bytes!("silence_1500ms.mp3");

/// TTS configuration.
pub struct TtsConfig {
    pub google_api_key: String,
    pub voice: String,
    /// Pronunciation substitutions applied to the text before chunking and
    /// SSML wrapping. Defaults to `lexicon::default_lexicon()`; pass an
    /// explicit empty vec to disable.
    pub lexicon: Vec<crate::lexicon::LexiconEntry>,
}

impl TtsConfig {
    pub fn new(google_api_key: String) -> Self {
        Self {
            google_api_key,
            voice: "en-US-Chirp3-HD-Puck".to_string(),
            lexicon: crate::lexicon::default_lexicon(),
        }
    }

    pub fn with_voice(mut self, voice: String) -> Self {
        self.voice = voice;
        self
    }

    pub fn with_lexicon(mut self, lexicon: Vec<crate::lexicon::LexiconEntry>) -> Self {
        self.lexicon = lexicon;
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
    cache_dir: Option<String>,
) -> Result<TtsResult> {
    // Lexicon runs before section parsing so that substitutions in header
    // lines (e.g. "## Pkl primer") flow into the section titles used for
    // chapter markers too.
    let substituted = if config.lexicon.is_empty() {
        None
    } else {
        Some(crate::lexicon::apply(text, &config.lexicon))
    };
    let effective_text: &str = substituted.as_deref().unwrap_or(text);
    let sections_text = parse_sections(effective_text);
    let has_sections = !sections_text.is_empty() && sections_text.iter().any(|s| s.title.is_some());
    let sections_for_chunking = if sections_text.is_empty() {
        vec![SectionText {
            title: None,
            body: effective_text.to_string(),
        }]
    } else {
        sections_text
    };

    let chunks = build_chunks(&sections_for_chunking, MAX_CHUNK_CHARS);
    let total_chunks = chunks.len();
    let word_count = text.split_whitespace().count();
    // Chunk metadata needed after the stream consumes `chunks`.
    let chunk_section_idxs: Vec<usize> = chunks.iter().map(|c| c.section_idx).collect();
    let chunk_append_pauses: Vec<bool> = chunks.iter().map(|c| c.append_pause).collect();

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

    if let Some(ref dir) = cache_dir {
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            tracing::warn!("Failed to create TTS chunk cache dir {dir}: {e}");
        }
    }

    let results: Vec<Result<(usize, Bytes, f64)>> = stream::iter(chunks.into_iter().enumerate())
        .map(|(i, chunk)| {
            let client = client.clone();
            let completed = completed.clone();
            let on_progress = on_progress.clone();
            let cache_dir = cache_dir.clone();
            async move {
                let chunk_words = chunk.text.split_whitespace().count();
                let cache_path = cache_dir.as_ref().map(|d| {
                    format!(
                        "{}/{}",
                        d,
                        chunk_cache_filename(i, &chunk.text, &config.voice)
                    )
                });
                let audio = if let Some(ref p) = cache_path {
                    match tokio::fs::read(p).await {
                        Ok(bytes) => {
                            tracing::info!(
                                "TTS chunk {}/{} reused from cache",
                                i + 1,
                                total_chunks
                            );
                            Some(Bytes::from(bytes))
                        }
                        Err(_) => None,
                    }
                } else {
                    None
                };
                let audio = match audio {
                    Some(a) => a,
                    None => {
                        tracing::info!(
                            "TTS chunk {}/{} ({chunk_words} words)",
                            i + 1,
                            total_chunks
                        );
                        let a = tts_google(&client, config, &chunk.text).await?;
                        if let Some(ref p) = cache_path {
                            let tmp = format!("{}.tmp", p);
                            if let Err(e) = tokio::fs::write(&tmp, &a).await {
                                tracing::warn!("Failed to write TTS chunk cache {p}: {e}");
                            } else if let Err(e) = tokio::fs::rename(&tmp, p).await {
                                tracing::warn!("Failed to rename TTS chunk cache {p}: {e}");
                            }
                        }
                        a
                    }
                };
                // Measure duration of the Google-returned MP3 before appending
                // any silence. mp3_duration can't walk past the ID3 tag at the
                // start of the silence file, so measuring the concatenation
                // returns a bogus ~1s duration — caller tracks pauses separately.
                let base_duration = mp3_duration::from_read(&mut std::io::Cursor::new(&audio[..]))
                    .map(|d| d.as_secs_f64())
                    .unwrap_or_else(|_| chunk_words as f64 / 150.0 * 60.0);
                let audio = if chunk.append_pause {
                    let mut combined = Vec::with_capacity(audio.len() + SECTION_SILENCE_MP3.len());
                    combined.extend_from_slice(&audio);
                    combined.extend_from_slice(SECTION_SILENCE_MP3);
                    Bytes::from(combined)
                } else {
                    audio
                };
                let done = completed.fetch_add(1, Ordering::SeqCst) + 1;
                if let Some(ref cb) = on_progress {
                    cb(done, total_chunks);
                }
                Ok::<_, anyhow::Error>((i, audio, base_duration))
            }
        })
        .buffer_unordered(TTS_CONCURRENCY)
        .collect()
        .await;

    let mut indexed: Vec<(usize, Bytes, f64)> = results.into_iter().collect::<Result<_>>()?;
    indexed.sort_by_key(|(i, _, _)| *i);
    let audio_parts: Vec<Bytes> = indexed.iter().map(|(_, b, _)| b.clone()).collect();

    // Per-chunk durations include the 1.5s pause for section-end chunks.
    const SECTION_PAUSE_SECS: f64 = 1.5;
    let per_chunk_durations: Vec<f64> = indexed
        .iter()
        .map(|(i, _, base)| {
            if chunk_append_pauses[*i] {
                base + SECTION_PAUSE_SECS
            } else {
                *base
            }
        })
        .collect();

    // Compute section start times by summing chunk durations before each
    // section's first chunk.
    let sections = if has_sections {
        build_section_timeline(
            &sections_for_chunking,
            &chunk_section_idxs,
            &per_chunk_durations,
        )
    } else {
        Vec::new()
    };

    // Concatenate MP3 chunks
    let total_bytes: Vec<u8> = audio_parts.iter().flat_map(|b| b.to_vec()).collect();
    let audio = Bytes::from(total_bytes);

    // Sum per-chunk durations rather than re-parsing the concatenated MP3:
    // the per-section silence MP3 carries an ID3 header that halts the frame
    // walker mid-stream, so parsing the whole file returns a bogus short
    // duration (~1s for Spanner-sized inputs).
    let duration_secs = per_chunk_durations.iter().sum::<f64>() as u32;

    Ok(TtsResult {
        audio,
        duration_secs,
        chunks_total: total_chunks,
        sections,
    })
}

/// Prepend an ID3v2.3 tag with CHAP + CTOC frames so podcast apps that read
/// embedded chapters (Overcast, Apple Podcasts pre-2025) show them. Clients
/// walk past the tag to find the first MP3 sync frame, so the returned bytes
/// are still playable.
pub fn embed_chapters(
    audio: &[u8],
    sections: &[Section],
    total_duration_secs: u32,
) -> Result<Bytes> {
    if sections.is_empty() {
        return Ok(Bytes::copy_from_slice(audio));
    }
    let total_ms = total_duration_secs.saturating_mul(1000);
    let mut tag = Tag::new();
    let mut element_ids: Vec<String> = Vec::with_capacity(sections.len());
    for (i, s) in sections.iter().enumerate() {
        let element_id = format!("ch{i}");
        element_ids.push(element_id.clone());
        let start_ms = (s.start_secs * 1000.0).max(0.0) as u32;
        let end_ms = sections
            .get(i + 1)
            .map(|n| (n.start_secs * 1000.0).max(0.0) as u32)
            .unwrap_or(total_ms);
        let title_frame = Frame::text("TIT2", s.title.clone());
        let chap = Chapter {
            element_id,
            start_time: start_ms,
            end_time: end_ms.max(start_ms),
            start_offset: u32::MAX,
            end_offset: u32::MAX,
            frames: vec![title_frame],
        };
        tag.add_frame(Frame::from(chap));
    }
    let toc = TableOfContents {
        element_id: "toc".to_string(),
        top_level: true,
        ordered: true,
        elements: element_ids,
        frames: Vec::new(),
    };
    tag.add_frame(Frame::from(toc));

    let mut buf: Vec<u8> = Vec::with_capacity(audio.len() + 1024);
    tag.write_to(&mut buf, Version::Id3v23)
        .context("Failed to write ID3v2 chapter tag")?;
    buf.extend_from_slice(audio);
    Ok(Bytes::from(buf))
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
        let rest = text[after_header..end].trim();
        let body = if rest.is_empty() {
            title.clone()
        } else {
            format!("{title}\n\n{rest}")
        };
        sections.push(SectionText {
            title: Some(title.clone()),
            body,
        });
    }
    sections
}

/// Cache filename for a chunk, keyed by index + content + voice so edits to the
/// source text or voice don't silently reuse stale audio.
fn chunk_cache_filename(index: usize, text: &str, voice: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    voice.hash(&mut hasher);
    text.hash(&mut hasher);
    format!("chunk-{:04}-{:016x}.mp3", index, hasher.finish())
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
        let fallback: String = sections
            .iter()
            .map(|s| s.body.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");
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
        .into_iter()
        .flat_map(split_long_sentence)
        .collect()
}

/// Split a sentence exceeding `MAX_SENTENCE_CHARS` at secondary punctuation
/// (`;`, `:`, `,`, or `\n`), converting the break into `. ` so Google TTS
/// parses the pieces as separate sentences. Falls back to a word-boundary
/// hard split if no secondary punctuation fits.
fn split_long_sentence(sentence: String) -> Vec<String> {
    if sentence.len() <= MAX_SENTENCE_CHARS {
        return vec![sentence];
    }
    let mut pieces = Vec::new();
    let mut current = String::new();
    let min_piece = MAX_SENTENCE_CHARS / 2;
    for ch in sentence.chars() {
        if matches!(ch, ';' | ':' | ',' | '\n') && current.trim_end().len() >= min_piece {
            let trimmed = current.trim_end().to_string();
            pieces.push(format!("{}. ", trimmed));
            current.clear();
            continue;
        }
        current.push(ch);
        if current.len() >= MAX_SENTENCE_CHARS {
            if let Some(cut) = rfind_word_boundary(&current, MAX_SENTENCE_CHARS) {
                let head: String = current.chars().take(cut).collect();
                let tail: String = current.chars().skip(cut).collect();
                pieces.push(format!("{}. ", head.trim_end()));
                current = tail.trim_start().to_string();
            }
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
    pieces.into_iter().flat_map(hard_split_if_needed).collect()
}

/// Word-boundary split for pieces still over the per-sentence limit.
fn hard_split_if_needed(piece: String) -> Vec<String> {
    if piece.len() <= MAX_SENTENCE_CHARS {
        return vec![piece];
    }
    let mut out = Vec::new();
    let mut remaining = piece;
    while remaining.len() > MAX_SENTENCE_CHARS {
        let cut = rfind_word_boundary(&remaining, MAX_SENTENCE_CHARS)
            .unwrap_or_else(|| remaining.chars().take(MAX_SENTENCE_CHARS).count());
        let head: String = remaining.chars().take(cut).collect();
        let tail: String = remaining.chars().skip(cut).collect();
        out.push(format!("{}. ", head.trim_end()));
        remaining = tail.trim_start().to_string();
    }
    if !remaining.is_empty() {
        out.push(remaining);
    }
    out
}

/// Return the (char-count) position of the last whitespace boundary whose
/// byte offset is ≤ `max_bytes`. None if no such boundary exists.
fn rfind_word_boundary(s: &str, max_bytes: usize) -> Option<usize> {
    let window_end = max_bytes.min(s.len());
    // Snap to a char boundary.
    let mut end = window_end;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let window = &s[..end];
    let byte_idx = window.rfind(char::is_whitespace)?;
    Some(s[..byte_idx].chars().count())
}

/// Wrap chunk text as SSML with `<p>` per paragraph and `<s>` per sentence.
/// Paragraphs are separated by blank lines in the source text. XML-special
/// characters in the sentence bodies are escaped.
///
/// Chirp3-HD voices support `<speak>`, `<p>`, and `<s>` but **not** `<mark>`
/// (per Google's docs). Adding `<mark>` here for transcript timepoint
/// alignment would silently break Chirp3-HD; fall back to Studio/Neural2
/// for that use case, or compute offsets externally.
fn build_ssml(text: &str) -> String {
    let mut out = String::from("<speak>");
    let mut wrote_paragraph = false;
    for raw_para in text.split("\n\n") {
        let para = raw_para.trim();
        if para.is_empty() {
            continue;
        }
        let mut sentences: Vec<String> = Vec::new();
        for s in split_sentences(para) {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                sentences.push(trimmed);
            }
        }
        if sentences.is_empty() {
            continue;
        }
        out.push_str("<p>");
        for s in sentences {
            out.push_str("<s>");
            out.push_str(&xml_escape(&s));
            out.push_str("</s>");
        }
        out.push_str("</p>");
        wrote_paragraph = true;
    }
    // `<speak/>` alone is invalid SSML; fall back to a single empty paragraph
    // so Google TTS returns a brief silence rather than 400.
    if !wrote_paragraph {
        out.push_str("<p><s></s></p>");
    }
    out.push_str("</speak>");
    out
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

async fn tts_google(client: &reqwest::Client, config: &TtsConfig, text: &str) -> Result<Bytes> {
    let url = format!(
        "https://texttospeech.googleapis.com/v1/text:synthesize?key={}",
        config.google_api_key
    );

    let ssml = build_ssml(text);

    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "input": { "ssml": ssml },
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

    let body: serde_json::Value = resp
        .json()
        .await
        .context("Google TTS response parse failed")?;
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
    fn test_split_sentences_breaks_long_run_on() {
        // Mimics the failing episode: a "sentence" with colons and commas but
        // no periods. Every output piece must be ≤ MAX_SENTENCE_CHARS.
        let segment = "Define the strategy by doing X across the enterprise";
        let long = format!(
            "Our strategic priorities are: {}",
            std::iter::repeat_n(segment, 40)
                .collect::<Vec<_>>()
                .join(", ")
        );
        assert!(long.len() > MAX_SENTENCE_CHARS);
        let pieces = split_sentences(&long);
        assert!(
            pieces.len() > 1,
            "expected multi-piece split, got {}",
            pieces.len()
        );
        for p in &pieces {
            assert!(
                p.len() <= MAX_SENTENCE_CHARS,
                "piece over limit: {} chars",
                p.len()
            );
        }
    }

    #[test]
    fn test_split_sentences_hard_split_no_punctuation() {
        // Run-on with no secondary punctuation at all — falls back to word
        // boundaries. Just needs to terminate and respect the limit.
        let long: String = "word ".repeat(400);
        assert!(long.len() > MAX_SENTENCE_CHARS);
        let pieces = split_sentences(&long);
        for p in &pieces {
            assert!(p.len() <= MAX_SENTENCE_CHARS);
        }
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
        assert_eq!(r[0].body, "Abstract\n\nAbstract body.");
        assert_eq!(r[1].title.as_deref(), Some("Introduction"));
        assert_eq!(r[1].body, "Introduction\n\nIntro body.");
    }

    #[test]
    fn test_parse_sections_with_preface() {
        let text = "Preamble.\n\n## Section One\n\nBody.";
        let r = parse_sections(text);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].title, None);
        assert_eq!(r[0].body, "Preamble.");
        assert_eq!(r[1].body, "Section One\n\nBody.");
        assert_eq!(r[1].title.as_deref(), Some("Section One"));
    }

    #[test]
    fn test_parse_sections_ignores_subheaders() {
        let text = "## Main\n\nBody.\n\n### Sub\n\nSubbody.";
        let r = parse_sections(text);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].title.as_deref(), Some("Main"));
        assert!(r[0].body.starts_with("Main\n\n"));
        assert!(r[0].body.contains("### Sub"));
    }

    #[test]
    fn test_build_chunks_marks_section_ends() {
        let sections = vec![
            SectionText {
                title: Some("A".into()),
                body: "Sentence one.".into(),
            },
            SectionText {
                title: Some("B".into()),
                body: "Sentence two.".into(),
            },
        ];
        let chunks = build_chunks(&sections, 1000);
        assert_eq!(chunks.len(), 2);
        assert!(
            chunks[0].append_pause,
            "end of non-final section should pause"
        );
        assert!(!chunks[1].append_pause, "final section does not pause");
    }

    #[test]
    fn test_embed_chapters_round_trip() {
        let audio = b"\xff\xfb\x90\x00fake-mp3-frames";
        let sections = vec![
            Section {
                title: "Intro".into(),
                start_secs: 0.0,
            },
            Section {
                title: "Middle".into(),
                start_secs: 12.5,
            },
            Section {
                title: "Outro".into(),
                start_secs: 42.0,
            },
        ];
        let out = embed_chapters(audio, &sections, 60).unwrap();
        assert_eq!(&out[..3], b"ID3", "must start with ID3v2 magic");
        let tag = Tag::read_from2(&mut std::io::Cursor::new(&out[..])).unwrap();
        let chaps: Vec<&Chapter> = tag.chapters().collect();
        assert_eq!(chaps.len(), 3);
        assert_eq!(chaps[0].start_time, 0);
        assert_eq!(chaps[0].end_time, 12_500);
        assert_eq!(chaps[1].start_time, 12_500);
        assert_eq!(chaps[1].end_time, 42_000);
        assert_eq!(chaps[2].start_time, 42_000);
        assert_eq!(chaps[2].end_time, 60_000);
    }

    #[test]
    fn test_embed_chapters_empty_passes_through() {
        let audio = b"\xff\xfb\x90\x00fake";
        let out = embed_chapters(audio, &[], 10).unwrap();
        assert_eq!(&out[..], audio);
    }

    #[test]
    fn test_build_ssml_wraps_paragraphs_and_sentences() {
        let ssml =
            build_ssml("First sentence. Second sentence.\n\nNew paragraph. With two sentences.");
        assert!(ssml.starts_with("<speak>"));
        assert!(ssml.ends_with("</speak>"));
        // Two paragraph blocks
        assert_eq!(ssml.matches("<p>").count(), 2);
        assert_eq!(ssml.matches("</p>").count(), 2);
        // Four sentences total
        assert_eq!(ssml.matches("<s>").count(), 4);
        assert_eq!(ssml.matches("</s>").count(), 4);
        assert!(ssml.contains("<s>First sentence.</s>"));
        assert!(ssml.contains("<s>New paragraph.</s>"));
    }

    #[test]
    fn test_build_ssml_escapes_xml_chars() {
        let ssml = build_ssml("Tom & Jerry said \"hi\" to <you>.");
        assert!(ssml.contains("&amp;"));
        assert!(ssml.contains("&quot;"));
        assert!(ssml.contains("&lt;you&gt;"));
        // Raw specials must not survive inside sentence bodies.
        assert!(!ssml.contains(" & "));
        assert!(!ssml.contains("<you>"));
    }

    #[test]
    fn test_build_ssml_empty_input() {
        // Empty-input degenerate case: must still produce syntactically valid
        // SSML (Google rejects bare `<speak/>`).
        let ssml = build_ssml("   \n\n   ");
        assert!(ssml.starts_with("<speak>") && ssml.ends_with("</speak>"));
        assert!(ssml.contains("<p>"));
    }

    #[test]
    fn test_build_ssml_single_paragraph() {
        let ssml = build_ssml("Just one sentence.");
        assert_eq!(ssml.matches("<p>").count(), 1);
        assert_eq!(ssml.matches("<s>").count(), 1);
        assert!(ssml.contains("<s>Just one sentence.</s>"));
    }

    #[test]
    fn test_build_section_timeline() {
        let sections = vec![
            SectionText {
                title: Some("A".into()),
                body: "x".into(),
            },
            SectionText {
                title: Some("B".into()),
                body: "y".into(),
            },
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
