use anyhow::{bail, Context, Result};
use base64::Engine;
use futures::stream::{self, StreamExt};

use crate::Document;

const FIRST_CHUNK_PROMPT: &str = r#"Extract all text from this academic paper for text-to-speech conversion.

Rules:
- Extract all text content in reading order (top-to-bottom, respecting column layout).
- For two-column layouts, complete the left column before the right column.
- Ignore page numbers, headers, footers, and running titles.
- Ignore figure captions and table captions — replace with "[Figure omitted]" or "[Table omitted]".
- Skip the bibliography / references section entirely.
- Skip appendices and supplementary material (anything after the conclusion, including sections titled "Appendix", "Supplementary", "Acknowledgments", etc.).
- If a chunk contains only bibliography or appendix content, output an empty result.
- Preserve paragraph breaks between pages.
- Output only the extracted text, nothing else.

At the very start of your output, on the first line only, write the paper's title prefixed with "TITLE: ". Then a blank line, then the full extracted text."#;

const CONTINUATION_PROMPT: &str = r#"Extract all text from this chunk of an academic paper for text-to-speech conversion.

Rules:
- Extract all text content in reading order (top-to-bottom, respecting column layout).
- For two-column layouts, complete the left column before the right column.
- Ignore page numbers, headers, footers, and running titles.
- Ignore figure captions and table captions — replace with "[Figure omitted]" or "[Table omitted]".
- Skip the bibliography / references section entirely.
- Skip appendices and supplementary material (anything after the conclusion, including sections titled "Appendix", "Supplementary", "Acknowledgments", etc.).
- If a chunk contains only bibliography or appendix content, output an empty result.
- Preserve paragraph breaks between pages.
- Output only the extracted text, nothing else. Do not include a title."#;

pub const DEFAULT_MODEL: &str = "gemini-flash-latest";

/// Pages per Gemini call. Smaller chunks run more in parallel and each call
/// finishes faster, which avoids long-running requests on large papers.
const CHUNK_PAGES: u32 = 4;
const CHUNK_CONCURRENCY: usize = 8;

pub async fn extract(pdf_path: &str, google_api_key: &str) -> Result<Document> {
    extract_with_model(pdf_path, google_api_key, DEFAULT_MODEL).await
}

pub async fn extract_with_model(
    pdf_path: &str,
    google_api_key: &str,
    model: &str,
) -> Result<Document> {
    let page_count = pdf_page_count(pdf_path).await?;
    tracing::info!(
        "Extracting PDF via Gemini ({page_count} pages, {CHUNK_PAGES}-page chunks)"
    );

    let work_dir = format!("{}_chunks", pdf_path.trim_end_matches(".pdf"));
    tokio::fs::create_dir_all(&work_dir).await?;

    let mut chunks: Vec<(u32, String)> = Vec::new();
    let mut start = 1u32;
    while start <= page_count {
        let end = (start + CHUNK_PAGES - 1).min(page_count);
        let chunk_path = format!("{}/chunk-{:04}.pdf", work_dir, start);
        split_chunk(pdf_path, start, end, &chunk_path).await?;
        chunks.push((start, chunk_path));
        start = end + 1;
    }

    let results: Vec<Result<(u32, String)>> = stream::iter(chunks.into_iter().enumerate())
        .map(|(i, (page_start, path))| {
            let api_key = google_api_key.to_string();
            let model = model.to_string();
            async move {
                let is_first = i == 0;
                let text = extract_chunk_cached(&path, &api_key, &model, is_first)
                    .await
                    .with_context(|| format!("Chunk starting at page {page_start}"))?;
                Ok::<_, anyhow::Error>((page_start, text))
            }
        })
        .buffer_unordered(CHUNK_CONCURRENCY)
        .collect()
        .await;

    let mut indexed: Vec<(u32, String)> = results.into_iter().collect::<Result<_>>()?;
    indexed.sort_by_key(|(p, _)| *p);

    let _ = tokio::fs::remove_dir_all(&work_dir).await;

    let (title, first_text) = match indexed.first() {
        Some((_, t)) => parse_title_and_text(t),
        None => bail!("No chunks produced from PDF"),
    };

    let mut pieces: Vec<String> = Vec::with_capacity(indexed.len());
    pieces.push(first_text);
    for (_, text) in indexed.into_iter().skip(1) {
        pieces.push(text.trim().to_string());
    }
    let raw_text = pieces.join("\n\n");

    if raw_text.is_empty() {
        bail!("Empty text extracted from PDF");
    }

    tracing::info!("Gemini extracted {} chars from {page_count} pages", raw_text.len());

    Ok(Document {
        title: Some(title),
        source_type: "pdf".to_string(),
        raw_text: Some(raw_text),
        ..Default::default()
    })
}

/// Remove any `*_chunks` working directories under `parent_dir` whose mtime is
/// older than `max_age`. Used to GC per-chunk caches left behind by jobs that
/// failed permanently before the success path could clean up.
pub async fn gc_chunk_dirs(parent_dir: &str, max_age: std::time::Duration) {
    let mut entries = match tokio::fs::read_dir(parent_dir).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("gc_chunk_dirs: cannot read {parent_dir}: {e}");
            return;
        }
    };
    let now = std::time::SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if !name_str.ends_with("_chunks") {
            continue;
        }
        let Ok(md) = entry.metadata().await else { continue };
        if !md.is_dir() {
            continue;
        }
        let mtime = md.modified().ok();
        let age = mtime.and_then(|t| now.duration_since(t).ok());
        if let Some(age) = age {
            if age > max_age {
                let path = entry.path();
                tracing::info!("GC: removing old chunk dir {path:?} (age {age:?})");
                if let Err(e) = tokio::fs::remove_dir_all(&path).await {
                    tracing::warn!("GC failed to remove {path:?}: {e}");
                }
            }
        }
    }
}

async fn pdf_page_count(pdf_path: &str) -> Result<u32> {
    let output = tokio::process::Command::new("pdfinfo")
        .arg(pdf_path)
        .output()
        .await
        .context("Failed to run pdfinfo — is poppler-utils installed?")?;
    if !output.status.success() {
        bail!("pdfinfo failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Pages:") {
            return rest.trim().parse::<u32>().context("Parse pdfinfo Pages");
        }
    }
    bail!("pdfinfo output did not include Pages: {stdout}");
}

async fn split_chunk(pdf_path: &str, first: u32, last: u32, out_path: &str) -> Result<()> {
    let tmp = format!("{}.pages", out_path);
    tokio::fs::create_dir_all(&tmp).await?;
    let pattern = format!("{}/p-%d.pdf", tmp);

    let sep = tokio::process::Command::new("pdfseparate")
        .args([
            "-f",
            &first.to_string(),
            "-l",
            &last.to_string(),
            pdf_path,
            &pattern,
        ])
        .output()
        .await
        .context("Failed to run pdfseparate")?;
    if !sep.status.success() {
        bail!("pdfseparate failed: {}", String::from_utf8_lossy(&sep.stderr));
    }

    let page_files: Vec<String> = (first..=last)
        .map(|p| format!("{}/p-{}.pdf", tmp, p))
        .collect();

    if page_files.len() == 1 {
        tokio::fs::rename(&page_files[0], out_path).await?;
    } else {
        let mut args = page_files.clone();
        args.push(out_path.to_string());
        let unite = tokio::process::Command::new("pdfunite")
            .args(&args)
            .output()
            .await
            .context("Failed to run pdfunite")?;
        if !unite.status.success() {
            bail!("pdfunite failed: {}", String::from_utf8_lossy(&unite.stderr));
        }
    }

    let _ = tokio::fs::remove_dir_all(&tmp).await;
    Ok(())
}

fn cache_path_for(chunk_path: &str) -> String {
    format!("{}.txt", chunk_path.trim_end_matches(".pdf"))
}

async fn extract_chunk_cached(
    chunk_path: &str,
    google_api_key: &str,
    model: &str,
    is_first: bool,
) -> Result<String> {
    let cache_path = cache_path_for(chunk_path);
    if let Ok(cached) = tokio::fs::read_to_string(&cache_path).await {
        tracing::info!("Reusing cached chunk extraction: {cache_path}");
        return Ok(cached);
    }
    let text = extract_chunk(chunk_path, google_api_key, model, is_first).await?;
    if let Err(e) = tokio::fs::write(&cache_path, &text).await {
        tracing::warn!("Failed to write chunk cache {cache_path}: {e}");
    }
    Ok(text)
}

async fn extract_chunk(
    chunk_path: &str,
    google_api_key: &str,
    model: &str,
    is_first: bool,
) -> Result<String> {
    let pdf_bytes = tokio::fs::read(chunk_path)
        .await
        .with_context(|| format!("Failed to read chunk {chunk_path}"))?;

    if pdf_bytes.len() > 50 * 1024 * 1024 {
        bail!(
            "Chunk {chunk_path} exceeds Gemini 50MB inline limit ({:.1} MB)",
            pdf_bytes.len() as f64 / (1024.0 * 1024.0)
        );
    }

    let pdf_b64 = base64::engine::general_purpose::STANDARD.encode(&pdf_bytes);
    let prompt = if is_first { FIRST_CHUNK_PROMPT } else { CONTINUATION_PROMPT };

    let request = serde_json::json!({
        "contents": [{
            "parts": [
                { "inline_data": { "mime_type": "application/pdf", "data": pdf_b64 } },
                { "text": prompt }
            ]
        }],
        "generationConfig": {
            "temperature": 0.0,
            "maxOutputTokens": 32768,
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;

    let url = format!(
        "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
        model, google_api_key
    );

    // Retry transient errors (5xx, 429, network). Short backoff — long backoffs
    // (e.g. quota cooldowns) are handled at the job level, where the per-chunk
    // cache lets sibling chunks' work survive across job retries.
    let backoffs_ms = [0u64, 2_000, 8_000];
    for (attempt, delay_ms) in backoffs_ms.iter().enumerate() {
        if *delay_ms > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(*delay_ms)).await;
        }
        match try_extract_chunk(&client, &url, &request).await {
            Ok(text) => return Ok(text),
            Err(ChunkError { transient, source }) => {
                if !transient || attempt + 1 == backoffs_ms.len() {
                    return Err(source);
                }
                tracing::warn!(
                    "Gemini chunk attempt {} failed (transient): {source}",
                    attempt + 1
                );
            }
        }
    }
    unreachable!("retry loop returns on every branch")
}

struct ChunkError {
    transient: bool,
    source: anyhow::Error,
}

async fn try_extract_chunk(
    client: &reqwest::Client,
    url: &str,
    request: &serde_json::Value,
) -> std::result::Result<String, ChunkError> {
    let resp = client
        .post(url)
        .json(request)
        .send()
        .await
        .map_err(|e| ChunkError {
            transient: true,
            source: anyhow::Error::new(e).context("Gemini request failed"),
        })?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        // 5xx and 429 are transient; 4xx (other) are permanent.
        let transient = status.is_server_error() || status.as_u16() == 429;
        return Err(ChunkError {
            transient,
            source: anyhow::anyhow!("Gemini API failed ({status}): {body}"),
        });
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| ChunkError {
        transient: true,
        source: anyhow::Error::new(e).context("Gemini response JSON parse failed"),
    })?;

    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .ok_or_else(|| ChunkError {
            transient: false,
            source: anyhow::anyhow!("No parts in Gemini response"),
        })?;

    let text: String = parts
        .iter()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    if text.is_empty() {
        let finish_reason = body["candidates"][0]["finishReason"]
            .as_str()
            .unwrap_or("unknown");
        return Err(ChunkError {
            transient: false,
            source: anyhow::anyhow!("Empty text from Gemini chunk (finishReason={finish_reason})"),
        });
    }

    Ok(text)
}

fn parse_title_and_text(full: &str) -> (String, String) {
    if let Some(rest) = full.strip_prefix("TITLE: ") {
        if let Some(newline_idx) = rest.find('\n') {
            let title = rest[..newline_idx].trim().to_string();
            let text = rest[newline_idx..].trim_start().to_string();
            return (title, text);
        }
    }
    ("Untitled PDF".to_string(), full.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_title_and_text_with_prefix() {
        let input = "TITLE: Spanner: Google's Database\n\nAbstract\nThis paper describes...";
        let (title, text) = parse_title_and_text(input);
        assert_eq!(title, "Spanner: Google's Database");
        assert!(text.starts_with("Abstract"));
    }

    #[test]
    fn test_parse_title_and_text_without_prefix() {
        let input = "No title prefix here. Just content.";
        let (title, text) = parse_title_and_text(input);
        assert_eq!(title, "Untitled PDF");
        assert_eq!(text, input);
    }
}
