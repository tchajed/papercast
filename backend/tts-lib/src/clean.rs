use anyhow::{Context, Result};
use futures::stream::{self, StreamExt};
use futures::FutureExt;
use serde::Deserialize;

use crate::{Document, Provider, Usage};

// ---------------------------------------------------------------------------
// Single-call path (short articles, and fallback if outline fails).
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

    let claude_model = "claude-sonnet-4-6";

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
// Chunked path: Haiku outline → plan chunks (possibly sub-splitting long
// sections at paragraph boundaries) → parallel per-chunk clean.
// ---------------------------------------------------------------------------

const OUTLINE_SYSTEM_PROMPT: &str = r#"You are analyzing an academic paper to prepare it for chunked text-to-speech cleanup. You will not rewrite the text; you only identify section boundaries and write a short spoken introduction.

Return a single JSON object with these fields:

- "intro_line": a single natural sentence that will be spoken as the opening of the podcast, naming the paper title, the publication venue if discernible, and a brief summary of the authors. Examples:
    "We're looking at 'Attention Is All You Need', published at NeurIPS 2017, by Ashish Vaswani and seven co-authors from Google Brain."
    "Today's paper is 'Spanner: Google's Globally-Distributed Database', from OSDI 2012, by a team of Google engineers."
  If author or venue info is missing, omit that part naturally. If there is not enough information to produce any useful intro, return null.

- "sections": an array of objects, one per MAJOR TOP-LEVEL section of the paper's main body (Abstract, Introduction, Methods, Results, Discussion, Conclusion, etc.). Do NOT include subsections like "2.3 Geometry" or "Section 4.1" — only top-level sections. Each object has:
    - "title": the section's own name, capitalized, with numbering stripped (e.g. "Introduction", not "1. Introduction").
    - "start_anchor": an EXACT substring copied verbatim from the input, 25-80 characters long, from a SINGLE LINE of the input — no embedded newlines or tabs. It must uniquely locate where the section's REAL PROSE BODY begins in the raw text. If the document has a table of contents, index, or list of figures at the start, your anchor MUST NOT come from there — pick a phrase from the actual flowing body paragraph of the section, which appears later in the document. A short heading like "Part 2: Fixing government" is a bad anchor because it likely appears in both the TOC and the body; prefer a distinctive phrase from the first or second body sentence. COPY CHARACTER-BY-CHARACTER; do not paraphrase, reformat whitespace, or correct encoding artifacts. If nothing 25 characters long is verbatim, use the longest verbatim fragment you can find (minimum 15 characters).

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
enum Role {
    Open,
    Continue,
    Close,
}

fn chunk_system_prompt(role: Role) -> String {
    let preface = match role {
        Role::Open => {
            "You are cleaning the opening of an academic paper for text-to-speech. \
             A spoken introduction naming the paper's title, venue, and authors is prepended \
             separately by the caller — do NOT restate the title or author list. \
             Just clean this text so it flows naturally from that introduction."
        }
        Role::Continue => {
            "You are cleaning a middle portion of an academic paper for text-to-speech. \
             Assume listeners have heard earlier content and will hear later content. \
             Do not add a preamble, recap, or sign-off."
        }
        Role::Close => {
            "You are cleaning the final portion of an academic paper's main body for text-to-speech. \
             End on the text's natural final sentence — do not add an outro or sign-off, \
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

    let body = result.text.trim();
    let body = body
        .strip_prefix("```json")
        .or_else(|| body.strip_prefix("```"))
        .unwrap_or(body);
    let body = body.strip_suffix("```").unwrap_or(body);
    let body = body.trim();

    let outline: Outline = serde_json::from_str(body).with_context(|| {
        format!("Outline JSON parse failed. Raw response:\n{}", result.text)
    })?;
    Ok((outline, result.usage))
}

/// Find `anchor` in `haystack`. Tries an exact match, then falls back to
/// progressively shorter verbatim prefixes: Haiku occasionally paraphrases
/// the tail when asked to copy verbatim.
fn find_anchor(haystack: &str, anchor: &str) -> Option<usize> {
    find_anchor_with(haystack, anchor, |h, n| h.find(n))
}

fn rfind_anchor(haystack: &str, anchor: &str) -> Option<usize> {
    find_anchor_with(haystack, anchor, |h, n| h.rfind(n))
}

fn find_anchor_with<F>(haystack: &str, anchor: &str, locate: F) -> Option<usize>
where
    F: Fn(&str, &str) -> Option<usize>,
{
    if let Some(off) = locate(haystack, anchor) {
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
        if let Some(off) = locate(haystack, prefix) {
            return Some(off);
        }
    }
    None
}

/// Split raw_text into (title, slice) pairs using the outline's anchors.
fn locate_sections<'a>(raw_text: &'a str, outline: &Outline) -> Result<Vec<(String, &'a str)>> {
    if outline.sections.is_empty() {
        anyhow::bail!("outline returned no sections");
    }

    // Forward-walk section anchors so TOC-duplicate phrases naturally get
    // skipped as later sections push past them.
    let mut starts: Vec<(usize, &str)> = Vec::with_capacity(outline.sections.len());
    let mut cursor = 0usize;
    for s in &outline.sections {
        let search_region = &raw_text[cursor..];
        let rel = find_anchor(search_region, &s.start_anchor).with_context(|| {
            format!(
                "section anchor not found for {:?} (anchor: {:?})",
                s.title, s.start_anchor
            )
        })?;
        let abs = cursor + rel;
        starts.push((abs, s.title.as_str()));
        cursor = abs + s.start_anchor.len().min(search_region.len() - rel);
    }

    // End anchor: only honor it if AFTER the last section start (otherwise it
    // likely matched a TOC entry and would truncate the whole body).
    let last_section_start = starts.last().map(|(o, _)| *o).unwrap_or(0);
    let end_offset = outline
        .main_body_end_anchor
        .as_deref()
        .and_then(|a| rfind_anchor(raw_text, a))
        .filter(|&off| off > last_section_start)
        .unwrap_or(raw_text.len());
    let body = &raw_text[..end_offset];

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

    // Guardrail: tiny slices mean anchors matched TOC entries, not bodies.
    const MIN_SECTION_CHARS: usize = 300;
    if let Some((title, slice)) = slices
        .iter()
        .find(|(_, s)| s.trim().len() < MIN_SECTION_CHARS)
    {
        anyhow::bail!(
            "section {:?} slice is only {} chars; anchors likely matched TOC",
            title,
            slice.trim().len()
        );
    }

    Ok(slices)
}

// ---------------------------------------------------------------------------
// Chunk planning: split long sections at paragraph boundaries.
// ---------------------------------------------------------------------------

/// Target max chars per chunk sent to the cleanup model. Kept well below
/// per-call output caps so a chunk's cleaned output never truncates.
const TARGET_CHUNK_CHARS: usize = 25_000;

/// Chars of previous chunk's tail to pass as context so the model can
/// maintain voice and avoid re-introducing concepts mid-section.
const PREV_TAIL_CHARS: usize = 400;

struct Chunk {
    title: String,
    text: String,
    is_section_start: bool,
    role: Role,
    prev_tail: Option<String>,
}

fn plan_chunks(sections: Vec<(String, &str)>) -> Vec<Chunk> {
    let mut raw: Vec<(String, String, bool)> = Vec::new(); // (title, text, is_section_start)
    for (title, slice) in sections {
        let pieces = split_section(slice, TARGET_CHUNK_CHARS);
        for (idx, piece) in pieces.into_iter().enumerate() {
            raw.push((title.clone(), piece, idx == 0));
        }
    }

    let n = raw.len();
    let mut chunks: Vec<Chunk> = Vec::with_capacity(n);
    let mut prev_text: Option<String> = None;
    for (i, (title, text, is_section_start)) in raw.into_iter().enumerate() {
        let role = if i == 0 {
            Role::Open
        } else if i + 1 == n {
            Role::Close
        } else {
            Role::Continue
        };
        let prev_tail = if is_section_start {
            None
        } else {
            prev_text.as_deref().map(|t| tail(t, PREV_TAIL_CHARS))
        };
        prev_text = Some(text.clone());
        chunks.push(Chunk {
            title,
            text,
            is_section_start,
            role,
            prev_tail,
        });
    }
    chunks
}

/// Split `section` into pieces each ≤ target chars, preferring paragraph
/// boundaries ("\n\n"), then sentence boundaries for oversized paragraphs.
fn split_section(section: &str, target: usize) -> Vec<String> {
    if section.len() <= target {
        return vec![section.trim().to_string()];
    }

    let paragraphs: Vec<&str> = section.split("\n\n").collect();
    let mut pieces: Vec<String> = Vec::new();
    let mut cur = String::new();
    for para in paragraphs {
        let para = para.trim_matches('\n');
        if para.is_empty() {
            continue;
        }

        // Oversized paragraph: flush current, then sentence-split it.
        if para.len() > target {
            if !cur.is_empty() {
                pieces.push(std::mem::take(&mut cur).trim().to_string());
            }
            for sent_group in split_by_sentences(para, target) {
                pieces.push(sent_group);
            }
            continue;
        }

        if cur.len() + para.len() + 2 > target && !cur.is_empty() {
            pieces.push(std::mem::take(&mut cur).trim().to_string());
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    if !cur.is_empty() {
        pieces.push(cur.trim().to_string());
    }
    pieces.into_iter().filter(|p| !p.is_empty()).collect()
}

/// Greedy sentence-boundary split (". " as a proxy) for paragraphs larger
/// than `target`. Falls back to a hard char split if no sentence boundary
/// fits.
fn split_by_sentences(para: &str, target: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut remaining = para;
    while remaining.len() > target {
        // Find the last ". " before `target`, skipping abbreviations only
        // to the extent that a simple boundary is "good enough" here.
        let window = &remaining[..target];
        let cut = window
            .rfind(". ")
            .map(|i| i + 2)
            .or_else(|| window.rfind('\n').map(|i| i + 1))
            .unwrap_or_else(|| {
                // No boundary: hard-cut at target, but snap to a char boundary.
                let mut c = target;
                while !remaining.is_char_boundary(c) && c > 0 {
                    c -= 1;
                }
                c
            });
        pieces.push(remaining[..cut].trim().to_string());
        remaining = remaining[cut..].trim_start();
    }
    if !remaining.is_empty() {
        pieces.push(remaining.trim().to_string());
    }
    pieces
}

fn tail(s: &str, n_chars: usize) -> String {
    let total = s.chars().count();
    if total <= n_chars {
        return s.trim().to_string();
    }
    let skip = total - n_chars;
    s.chars().skip(skip).collect::<String>().trim().to_string()
}

// ---------------------------------------------------------------------------
// Per-chunk cleanup.
// ---------------------------------------------------------------------------

const CHUNK_CONCURRENCY: usize = 4;

/// Compute max_tokens for a chunk. Cleaning preserves length roughly, so we
/// need output headroom ≥ input tokens. Using ~3 chars/token as a safe
/// lower bound, plus a 2k margin, clamped to the per-model 32k ceiling we
/// use everywhere.
fn max_output_tokens_for(input_chars: usize) -> u32 {
    let estimated = (input_chars / 3) as u32 + 2048;
    estimated.clamp(4096, 32768)
}

async fn clean_one(provider: Provider, chunk: Chunk) -> Result<(String, Usage, bool)> {
    // Haiku handles mechanical cleanup (citations, figures, LaTeX, abbrevs)
    // well. Escalate to Sonnet for math-heavy text where equation paraphrasing
    // benefits from stronger judgment.
    let model = if is_math_heavy(&chunk.text) {
        "claude-sonnet-4-6"
    } else {
        "claude-haiku-4-5"
    };
    let system = chunk_system_prompt(chunk.role);

    let user_message = match chunk.prev_tail.as_deref() {
        Some(tail) if !tail.is_empty() => format!(
            "Previous chunk ended with (context only, do not repeat):\n---\n{tail}\n---\n\nClean the following text, continuing seamlessly from the previous chunk:\n\n{}",
            chunk.text
        ),
        _ => chunk.text.clone(),
    };

    let max_tokens = max_output_tokens_for(user_message.len());
    let client = reqwest::Client::new();

    // One retry on transient errors (502/503/504/timeout).
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 0..2 {
        match provider
            .chat_opts(
                &client,
                model,
                Some(&system),
                &user_message,
                max_tokens,
                true,
            )
            .await
        {
            Ok(result) => return Ok((result.text, result.usage, chunk.is_section_start)),
            Err(e) => {
                let msg = format!("{e:#}");
                let transient = msg.contains("502")
                    || msg.contains("503")
                    || msg.contains("504")
                    || msg.contains("timed out")
                    || msg.contains("timeout");
                if attempt == 0 && transient {
                    tracing::warn!("per-chunk transient error, retrying: {msg}");
                    last_err = Some(e);
                    continue;
                }
                return Err(e).context("per-chunk clean call failed");
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("per-chunk retry loop exhausted")))
}

async fn clean_chunked(
    doc: &Document,
    provider: &Provider,
    raw_text: &str,
) -> Result<(Document, Vec<Usage>)> {
    let (outline, outline_usage) = run_outline(provider, raw_text).await?;
    tracing::info!(
        "Outline: intro={} sections={} end_anchor={:?} titles={:?}",
        outline.intro_line.is_some(),
        outline.sections.len(),
        outline.main_body_end_anchor.as_deref(),
        outline.sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>()
    );

    let sections = locate_sections(raw_text, &outline)?;
    let chunks = plan_chunks(sections);
    let n = chunks.len();
    tracing::info!(
        "Planned {n} chunks across {} sections",
        chunks.iter().filter(|c| c.is_section_start).count()
    );

    let results: Vec<Result<(usize, String, String, Usage, bool)>> =
        stream::iter(chunks.into_iter().enumerate())
            .map(|(i, chunk)| {
                let provider = provider.clone();
                let title = chunk.title.clone();
                async move {
                    let (text, usage, is_start) = clean_one(provider, chunk).await?;
                    Ok::<_, anyhow::Error>((i, title, text, usage, is_start))
                }
                .boxed()
            })
            .buffer_unordered(CHUNK_CONCURRENCY)
            .collect()
            .await;

    let mut cleaned: Vec<(usize, String, String, bool)> = Vec::with_capacity(n);
    let mut usages: Vec<Usage> = Vec::with_capacity(n + 1);
    usages.push(outline_usage);
    for r in results {
        let (i, title, text, usage, is_start) = r?;
        usages.push(usage);
        cleaned.push((i, title, text, is_start));
    }
    cleaned.sort_by_key(|(i, _, _, _)| *i);

    let mut out = String::new();
    if let Some(intro) = outline.intro_line.as_deref().map(str::trim) {
        if !intro.is_empty() {
            out.push_str(intro);
            out.push_str("\n\n");
        }
    }
    for (_, title, text, is_start) in &cleaned {
        let body = text.trim();
        if body.is_empty() {
            continue;
        }
        if *is_start {
            out.push_str("## ");
            out.push_str(title);
            out.push_str("\n\n");
        }
        out.push_str(body);
        out.push_str("\n\n");
    }
    let cleaned_text = out.trim_end().to_string();
    let word_count = cleaned_text.split_whitespace().count();
    tracing::info!(
        "Cleaning complete (chunked): {word_count} words, {n} chunks"
    );

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
/// outline pass to split into sections, sub-splits long sections at paragraph
/// boundaries, and cleans chunks in parallel. Articles — and any case where
/// the outline or anchor-location fails — fall back to a single-call cleanup.
///
/// TODO: If a single chunk fails, we currently fail the whole clean job and
/// retry from scratch. Consider promoting chunks to DB-level jobs for
/// finer-grained retry once we see how this performs in practice.
///
/// TODO: Monitor cleanup quality on the Haiku-default path vs. the previous
/// Sonnet-only flow — missed citation/figure removals, awkward equation
/// paraphrasing in non-math-heavy sections, over-summarization. If quality
/// drops, flip the per-chunk default to Sonnet in `clean_one`.
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
