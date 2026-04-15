use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use serde::Deserialize;

use crate::{Document, Provider, Usage};

// ---------------------------------------------------------------------------
// Single-call path (still used for short articles and as a fallback if the
// Haiku outline pass fails or produces anchors we can't locate).
// ---------------------------------------------------------------------------

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

const ARTICLE_HEADER_RULE: &str = r#"
- This is a long article. Mark each major section with a markdown header line
  of the form `## Section Title` on its own line, blank line before and after.
  Use the article's own section names when present. Do not add subsection
  headers (no `###`). If the article has no clear section structure, omit
  headers entirely."#;

const LONG_ARTICLE_WORD_THRESHOLD: usize = 5000;

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
- Omit the bibliography / references section entirely.
- Omit appendices, supplementary material, and acknowledgments (everything after the conclusion).
- Keep all substantive content from the main body — do not summarize or omit findings, methods, or discussion.
- Mark each major section with a markdown header line of the form
  `## Section Title` on its own line, blank line before and after. Use the
  paper's own section names (e.g. "Abstract", "Introduction", "Methods",
  "Results", "Discussion", "Conclusion"). Do not include the numbering.
  Do not add subsection headers (no `###`). If the paper has no clear
  section structure, omit headers entirely.
- Output only the cleaned text, nothing else."#;

fn is_math_heavy(text: &str) -> bool {
    let words = text.split_whitespace().count().max(1);
    let backslash_cmds = text.matches('\\').count();
    let math_symbols = text
        .chars()
        .filter(|c| {
            matches!(*c,
                '∑' | '∫' | '∂' | '∇' | '∞' | '≤' | '≥' | '≠' | '≈' | '→' | '⇒' | '⊆' | '⊇' | '∈' | '∉' | '∀' | '∃' | '⋅' | '×' | '±'
            ) || matches!(*c as u32, 0x0391..=0x03C9)
        })
        .count();
    let density = (backslash_cmds + math_symbols) as f64 / words as f64 * 1000.0;
    density > 15.0
}

async fn clean_single(
    doc: &Document,
    provider: &Provider,
    raw_text: &str,
) -> Result<(Document, Usage)> {
    let system_prompt: std::borrow::Cow<'static, str> = match doc.source_type.as_str() {
        "arxiv" | "pdf" => ACADEMIC_SYSTEM_PROMPT.into(),
        _ => {
            let word_count = raw_text.split_whitespace().count();
            if word_count > LONG_ARTICLE_WORD_THRESHOLD {
                format!("{ARTICLE_SYSTEM_PROMPT}{ARTICLE_HEADER_RULE}").into()
            } else {
                ARTICLE_SYSTEM_PROMPT.into()
            }
        }
    };

    let claude_model = match doc.source_type.as_str() {
        "arxiv" | "pdf" if is_math_heavy(raw_text) => "claude-opus-4-6",
        _ => "claude-sonnet-4-6",
    };

    let client = reqwest::Client::new();
    let result = provider
        .chat(&client, claude_model, Some(&system_prompt), raw_text, 32768)
        .await?;
    let cleaned_text = result.text;

    let word_count = cleaned_text.split_whitespace().count();
    tracing::info!("Cleaning complete (single-call): {word_count} words");

    Ok((
        Document {
            cleaned_text: Some(cleaned_text),
            word_count: Some(word_count),
            ..doc.clone()
        },
        result.usage,
    ))
}

// ---------------------------------------------------------------------------
// Chunked path (academic sources): Haiku outline → parallel per-section clean.
// ---------------------------------------------------------------------------

const OUTLINE_SYSTEM_PROMPT: &str = r#"You are analyzing an academic paper to prepare it for chunked text-to-speech cleanup. You will not rewrite the text; you only identify section boundaries and write a short spoken introduction.

Return a single JSON object with these fields:

- "intro_line": a single natural sentence that will be spoken as the opening of the podcast, naming the paper title, the publication venue if discernible, and a brief summary of the authors. Examples:
    "We're looking at 'Attention Is All You Need', published at NeurIPS 2017, by Ashish Vaswani and seven co-authors from Google Brain."
    "Today's paper is 'Spanner: Google's Globally-Distributed Database', from OSDI 2012, by a team of Google engineers."
  If author or venue info is missing, omit that part naturally. If there is not enough information to produce any useful intro, return null.

- "sections": an array of objects, one per MAJOR TOP-LEVEL section of the paper's main body (Abstract, Introduction, Methods, Results, Discussion, Conclusion, etc.). Do NOT include subsections like "2.3 Geometry" or "Section 4.1" — only top-level sections. Each object has:
    - "title": the section's own name, capitalized, with numbering stripped (e.g. "Introduction", not "1. Introduction").
    - "start_anchor": an EXACT substring copied verbatim from the input, 25-80 characters long, from a SINGLE LINE of the input — no embedded newlines or tabs. It must uniquely locate where the section begins in the raw text. The anchor can be the section heading (e.g. "1. Introduction"), the first phrase of the section's body (e.g. "We introduce a system that"), or anything in between — pick whichever produces the cleanest single-line verbatim fragment. COPY CHARACTER-BY-CHARACTER; do not paraphrase, reformat whitespace, or correct encoding artifacts. If nothing 25 characters long is verbatim, use the longest verbatim fragment you can find (minimum 15 characters).

- "main_body_end_anchor": an EXACT substring (25-80 chars), single-line, copied from the first line of the bibliography / references / appendix / acknowledgments — whatever marks the end of the paper's main readable content. Everything at and after this anchor will be dropped. If the paper has no such trailing content, return null.

Output ONLY the JSON object, no markdown fences, no commentary."#;

const CHUNK_ACADEMIC_RULES: &str = r#"Rules:
- Remove all citation markers: [1], [23], (Smith et al., 2019), etc.
- Remove figure and table references: "as shown in Figure 3" → omit entirely.
- Rewrite inline equations as spoken English:
    \frac{a}{b} → "a over b"
    x^2 → "x squared"
    \sum_{i=1}^{n} → "the sum from i equals 1 to n of"
    For complex equations, describe what they compute rather than reading symbol-by-symbol.
- Expand abbreviations on first use if the expansion aids comprehension.
- Replace "in the next section" / "as mentioned above" with brief inline context.
- Remove LaTeX artifacts, section numbering, footnote markers.
- Keep all substantive content — do not summarize or omit findings, methods, or discussion.
- Do NOT emit a section heading line. The section title is inserted separately by the caller.
- Output only the cleaned section text, nothing else."#;

#[derive(Copy, Clone, Debug)]
enum ChunkPosition {
    Intro,
    Mid,
    Final,
}

fn chunk_system_prompt(position: ChunkPosition) -> String {
    let preface = match position {
        ChunkPosition::Intro => {
            "You are cleaning the opening section of an academic paper for text-to-speech. \
             A spoken introduction naming the paper's title, venue, and authors is prepended \
             separately by the caller — do NOT restate the title or author list. \
             Just clean this section's text and let it flow naturally from the introduction."
        }
        ChunkPosition::Mid => {
            "You are cleaning an interior section of an academic paper for text-to-speech. \
             Assume listeners have heard earlier sections and will hear later ones. \
             Do not add a preamble or recap."
        }
        ChunkPosition::Final => {
            "You are cleaning the final section of an academic paper's main body for text-to-speech. \
             End on the section's natural final sentence — do not add an outro or sign-off, \
             but do not cut off mid-thought. This is the last spoken content, so it should \
             close cleanly."
        }
    };
    format!("{preface}\n\n{CHUNK_ACADEMIC_RULES}")
}

#[derive(Deserialize, Debug)]
struct Outline {
    #[serde(default)]
    intro_line: Option<String>,
    sections: Vec<OutlineSection>,
    #[serde(default)]
    main_body_end_anchor: Option<String>,
}

#[derive(Deserialize, Debug)]
struct OutlineSection {
    title: String,
    start_anchor: String,
}

async fn run_outline(provider: &Provider, raw_text: &str) -> Result<(Outline, Usage)> {
    let client = reqwest::Client::new();
    let result = provider
        .chat(
            &client,
            "claude-haiku-4-5",
            Some(OUTLINE_SYSTEM_PROMPT),
            raw_text,
            4096,
        )
        .await
        .context("Outline (Haiku) call failed")?;

    // Strip stray markdown code fences if the model added them despite instructions.
    let body = result.text.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .unwrap_or(body);
    let body = body.strip_suffix("```").unwrap_or(body);
    let body = body.trim();

    let outline: Outline = serde_json::from_str(body).with_context(|| {
        format!(
            "Outline JSON parse failed. Raw response:\n{}",
            result.text
        )
    })?;
    Ok((outline, result.usage))
}

/// Find `anchor` in `haystack`. Tries an exact match, then falls back to
/// progressively shorter verbatim prefixes of the anchor, because Haiku
/// occasionally paraphrases the tail of an anchor when asked to copy verbatim.
fn find_anchor(haystack: &str, anchor: &str) -> Option<usize> {
    if let Some(off) = haystack.find(anchor) {
        return Some(off);
    }
    let anchor = anchor.trim_start();
    for n in [60, 40, 25, 15] {
        let prefix_end = anchor
            .char_indices()
            .nth(n)
            .map(|(i, _)| i)
            .unwrap_or(anchor.len());
        if prefix_end < 10 {
            continue;
        }
        let prefix = &anchor[..prefix_end];
        if let Some(off) = haystack.find(prefix) {
            return Some(off);
        }
    }
    None
}

/// Split raw_text into (title, slice) pairs using the outline's anchors.
/// Fails if any anchor cannot be located; caller falls back to single-call path.
fn locate_sections<'a>(raw_text: &'a str, outline: &Outline) -> Result<Vec<(String, &'a str)>> {
    let end_offset = outline
        .main_body_end_anchor
        .as_deref()
        .and_then(|a| find_anchor(raw_text, a))
        .unwrap_or(raw_text.len());
    let body = &raw_text[..end_offset];

    if outline.sections.is_empty() {
        anyhow::bail!("outline returned no sections");
    }

    let mut starts: Vec<(usize, &str)> = Vec::with_capacity(outline.sections.len());
    for s in &outline.sections {
        let off = find_anchor(body, &s.start_anchor).with_context(|| {
            format!(
                "section anchor not found for {:?} (anchor: {:?})",
                s.title, s.start_anchor
            )
        })?;
        starts.push((off, s.title.as_str()));
    }
    starts.sort_by_key(|(o, _)| *o);

    let mut slices = Vec::with_capacity(starts.len());
    for i in 0..starts.len() {
        let (start, title) = starts[i];
        let end = if i + 1 < starts.len() {
            starts[i + 1].0
        } else {
            body.len()
        };
        slices.push((title.to_string(), &body[start..end]));
    }
    Ok(slices)
}

const CHUNK_CONCURRENCY: usize = 4;
const CHUNK_MAX_OUTPUT_TOKENS: u32 = 16384;

async fn clean_chunk(
    provider: Provider,
    section_text: String,
    position: ChunkPosition,
) -> Result<(String, Usage)> {
    // Haiku handles the mechanical cleanup rules (citations, figure refs,
    // LaTeX artifacts, abbreviations) well. Escalate to Sonnet for math-heavy
    // sections where equation paraphrasing benefits from stronger judgment.
    let model = if is_math_heavy(&section_text) {
        "claude-sonnet-4-6"
    } else {
        "claude-haiku-4-5"
    };
    let system = chunk_system_prompt(position);
    let client = reqwest::Client::new();
    let result = provider
        .chat_opts(
            &client,
            model,
            Some(&system),
            &section_text,
            CHUNK_MAX_OUTPUT_TOKENS,
            true, // cache system prompt — shared across most chunks
        )
        .await
        .context("per-chunk clean call failed")?;
    Ok((result.text, result.usage))
}

async fn clean_chunked(
    doc: &Document,
    provider: &Provider,
    raw_text: &str,
) -> Result<(Document, Vec<Usage>)> {
    let (outline, outline_usage) = run_outline(provider, raw_text).await?;
    tracing::info!(
        "Outline: intro={} sections={} end_anchor={}",
        outline.intro_line.is_some(),
        outline.sections.len(),
        outline.main_body_end_anchor.is_some()
    );

    let sections = locate_sections(raw_text, &outline)?;
    let n = sections.len();
    tracing::info!("Located {n} sections for parallel cleanup");

    // Assign position per index: first = Intro, last = Final, others = Mid.
    // Degenerate case n == 1: treat as Intro (we still prepend intro_line).
    let tasks: Vec<(usize, String, String, ChunkPosition)> = sections
        .into_iter()
        .enumerate()
        .map(|(i, (title, slice))| {
            let pos = if i == 0 {
                ChunkPosition::Intro
            } else if i + 1 == n {
                ChunkPosition::Final
            } else {
                ChunkPosition::Mid
            };
            (i, title, slice.to_string(), pos)
        })
        .collect();

    let results: Vec<Result<(usize, String, String, Usage)>> = stream::iter(tasks)
        .map(|(i, title, slice, pos)| {
            let provider = provider.clone();
            async move {
                let (text, usage) = clean_chunk(provider, slice, pos).await?;
                Ok::<_, anyhow::Error>((i, title, text, usage))
            }
            .boxed()
        })
        .buffer_unordered(CHUNK_CONCURRENCY)
        .collect()
        .await;

    let mut cleaned: Vec<(usize, String, String)> = Vec::with_capacity(n);
    let mut usages: Vec<Usage> = Vec::with_capacity(n + 1);
    usages.push(outline_usage);
    for r in results {
        let (i, title, text, usage) = r?;
        usages.push(usage);
        cleaned.push((i, title, text));
    }
    cleaned.sort_by_key(|(i, _, _)| *i);

    let mut out = String::new();
    if let Some(intro) = outline.intro_line.as_deref().map(str::trim) {
        if !intro.is_empty() {
            out.push_str(intro);
            out.push_str("\n\n");
        }
    }
    for (_, title, text) in &cleaned {
        out.push_str("## ");
        out.push_str(title);
        out.push_str("\n\n");
        out.push_str(text.trim());
        out.push_str("\n\n");
    }
    let cleaned_text = out.trim_end().to_string();
    let word_count = cleaned_text.split_whitespace().count();
    tracing::info!("Cleaning complete (chunked): {word_count} words, {n} sections");

    Ok((
        Document {
            cleaned_text: Some(cleaned_text),
            word_count: Some(word_count),
            ..doc.clone()
        },
        usages,
    ))
}

// ---------------------------------------------------------------------------
// Public entry point.
// ---------------------------------------------------------------------------

/// Clean raw text for TTS. For academic sources (arxiv/pdf), uses a Haiku
/// outline pass to split into sections and cleans them in parallel. Articles,
/// and any case where the outline or anchor-location fails, fall back to a
/// single-call cleanup.
///
/// TODO: If a single chunk fails, we currently fail the whole clean job and
/// retry from scratch. Consider promoting chunks to DB-level jobs for
/// finer-grained retry once we see how this performs in practice.
///
/// TODO: Monitor cleanup quality on the Haiku-default path. Watch for
/// regressions vs. the previous Sonnet-only flow — particularly missed
/// citation/figure removals, awkward equation paraphrasing in non-math-heavy
/// sections (where is_math_heavy returns false but the section still has
/// some math), and any over-summarization. If quality drops, the easy lever
/// is flipping the per-chunk default back to Sonnet in clean_chunk().
pub async fn clean(doc: &Document, provider: &Provider) -> Result<(Document, Vec<Usage>)> {
    let raw_text = doc
        .raw_text
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No raw_text available for cleaning"))?;

    let is_academic = matches!(doc.source_type.as_str(), "arxiv" | "pdf");
    if is_academic {
        match clean_chunked(doc, provider, raw_text).await {
            Ok(result) => return Ok(result),
            Err(e) => {
                tracing::warn!(
                    "Chunked cleanup failed; falling back to single-call path: {e:#}"
                );
            }
        }
    }

    let (doc, usage) = clean_single(doc, provider, raw_text).await?;
    Ok((doc, vec![usage]))
}
