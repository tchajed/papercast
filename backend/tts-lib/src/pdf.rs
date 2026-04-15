use anyhow::{Context, Result};
use base64::Engine;
use futures::stream::{self, StreamExt};

use crate::claude;
use crate::{Document, Usage};

const PAGE_CONCURRENCY: usize = 4;

const PDF_SYSTEM_PROMPT: &str = r#"You are extracting text from a page of a PDF document for text-to-speech conversion.

Rules:
- Extract all text content in reading order (top-to-bottom, respecting column layout).
- For two-column layouts, complete the left column before the right column.
- Ignore page numbers, headers, footers, and running titles.
- Ignore figure captions and table captions — replace with "[Figure omitted]" or "[Table omitted]".
- Skip the bibliography / references section entirely.
- Skip appendices and supplementary material (anything after the conclusion, including sections titled "Appendix", "Supplementary", "Acknowledgments", etc.).
- If a page contains only bibliography or appendix content, output an empty result.
- Preserve paragraph breaks.
- Output only the extracted text, nothing else."#;

// DPIs to try in order. If a page is content-filtered, we re-render at the next DPI.
// Empirically, different resolutions can trigger different filter behavior.
const RETRY_DPIS: &[u32] = &[200, 150];

struct PageResult {
    text: String,
    skipped: bool,
    input_tokens: u32,
    output_tokens: u32,
}

/// Extract text from a PDF file using pdftoppm + Claude vision.
/// Uses multiple DPIs as a fallback when the content filter blocks a page.
pub async fn extract(pdf_path: &str, anthropic_api_key: &str) -> Result<(Document, Vec<Usage>)> {
    let base = pdf_path.trim_end_matches(".pdf");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()?;

    // Pre-render all DPIs upfront so parallel page tasks can try fallbacks independently
    let mut rendered: Vec<(u32, String, Vec<String>)> = Vec::new();
    for &dpi in RETRY_DPIS {
        let dir = format!("{}_pages_{}", base, dpi);
        let files = render_pdf(pdf_path, &dir, dpi).await?;
        tracing::info!("PDF rendered {} pages at {dpi} DPI", files.len());
        rendered.push((dpi, dir, files));
    }

    let primary_dir = rendered[0].1.clone();
    let num_pages = rendered[0].2.len();

    let fallback_dirs: Vec<(u32, String)> = rendered
        .iter()
        .skip(1)
        .map(|(d, dir, _)| (*d, dir.clone()))
        .collect();
    let primary_files = rendered[0].2.clone();

    let page_results: Vec<Result<(usize, String, u32, u32)>> = stream::iter(
        primary_files.into_iter().enumerate()
    )
    .map(|(i, primary_path)| {
        let client = client.clone();
        let fallback_dirs = fallback_dirs.clone();
        async move {
            let page_num = i + 1;
            let mut result =
                extract_page(&client, anthropic_api_key, &primary_path, page_num).await?;
            let mut total_in = result.input_tokens;
            let mut total_out = result.output_tokens;

            if result.skipped {
                for (dpi, fb_dir) in &fallback_dirs {
                    let fb_path = page_path_for(fb_dir, page_num);
                    tracing::info!("Retrying page {page_num} at {dpi} DPI");
                    result = extract_page(&client, anthropic_api_key, &fb_path, page_num).await?;
                    total_in += result.input_tokens;
                    total_out += result.output_tokens;
                    if !result.skipped {
                        break;
                    }
                }
            }

            if result.skipped {
                tracing::warn!("Page {page_num} blocked by content filter at all DPIs, skipping");
            }
            Ok::<_, anyhow::Error>((i, result.text, total_in, total_out))
        }
    })
    .buffer_unordered(PAGE_CONCURRENCY)
    .collect()
    .await;

    let mut indexed: Vec<(usize, String, u32, u32)> = page_results.into_iter().collect::<Result<_>>()?;
    indexed.sort_by_key(|(i, _, _, _)| *i);
    let mut usages: Vec<Usage> = Vec::new();
    let all_text: Vec<String> = indexed
        .into_iter()
        .map(|(_, t, in_tok, out_tok)| {
            if in_tok + out_tok > 0 {
                usages.push(Usage {
                    provider: "claude".into(),
                    model: "claude-sonnet-4-6".into(),
                    input_tokens: in_tok,
                    output_tokens: out_tok,
                });
            }
            t
        })
        .filter(|t| !t.is_empty())
        .collect();

    let _ = num_pages;
    let raw_text = all_text.join("\n\n");

    if raw_text.is_empty() {
        anyhow::bail!("Empty text extracted from PDF");
    }

    let (title, title_usage) = match extract_title(&client, anthropic_api_key, &raw_text).await {
        Ok((t, u)) => (t, Some(u)),
        Err(_) => ("Untitled PDF".to_string(), None),
    };
    if let Some(u) = title_usage {
        usages.push(u);
    }

    // Clean up
    let _ = tokio::fs::remove_dir_all(&primary_dir).await;
    for (_, dir) in &fallback_dirs {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }

    Ok((Document {
        title: Some(title),
        source_type: "pdf".to_string(),
        raw_text: Some(raw_text),
        ..Default::default()
    }, usages))
}

async fn render_pdf(pdf_path: &str, out_dir: &str, dpi: u32) -> Result<Vec<String>> {
    tokio::fs::create_dir_all(out_dir).await?;
    let output = tokio::process::Command::new("pdftoppm")
        .args([
            "-jpeg",
            "-r",
            &dpi.to_string(),
            pdf_path,
            &format!("{}/page", out_dir),
        ])
        .output()
        .await
        .context("Failed to run pdftoppm — is poppler-utils installed?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("pdftoppm failed: {stderr}");
    }

    let mut page_files: Vec<String> = Vec::new();
    let mut entries = tokio::fs::read_dir(out_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "jpg") {
            page_files.push(path.to_string_lossy().to_string());
        }
    }
    page_files.sort();

    if page_files.is_empty() {
        anyhow::bail!("No pages rendered from PDF");
    }
    Ok(page_files)
}

fn page_path_for(dir: &str, page_num: usize) -> String {
    // pdftoppm pads to at least 2 digits; for >=100 pages it uses 3, etc.
    // We try both common widths.
    for width in [2, 3, 4] {
        let candidate = format!("{}/page-{:0width$}.jpg", dir, page_num, width = width);
        if std::path::Path::new(&candidate).exists() {
            return candidate;
        }
    }
    format!("{}/page-{:02}.jpg", dir, page_num)
}

async fn extract_page(
    client: &reqwest::Client,
    api_key: &str,
    page_path: &str,
    page_num: usize,
) -> Result<PageResult> {
    let page_bytes = tokio::fs::read(page_path).await
        .with_context(|| format!("Failed to read page image {page_path}"))?;
    let page_b64 = base64::engine::general_purpose::STANDARD.encode(&page_bytes);

    tracing::info!("Extracting text from page {page_num}");

    let request = claude::Request {
        model: "claude-sonnet-4-6".to_string(),
        max_tokens: 4096,
        temperature: 0.0,
        system: Some(vec![claude::SystemBlock {
            block_type: "text".to_string(),
            text: PDF_SYSTEM_PROMPT.to_string(),
            cache_control: None,
        }]),
        messages: vec![claude::Message {
            role: "user".to_string(),
            content: claude::MessageContent::Blocks(vec![
                claude::ContentBlock::Image {
                    r#type: "image".to_string(),
                    source: claude::ImageSource {
                        r#type: "base64".to_string(),
                        media_type: "image/jpeg".to_string(),
                        data: page_b64,
                    },
                },
                claude::ContentBlock::Text {
                    r#type: "text".to_string(),
                    text: format!("Extract all text from page {page_num} of this document."),
                },
            ]),
        }],
    };

    let resp = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&request)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if body.contains("content filtering") {
            return Ok(PageResult {
                text: String::new(),
                skipped: true,
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        tracing::error!("Claude API error on page {page_num}: {status} {body}");
        anyhow::bail!("Claude API failed on page {page_num} ({status}): {body}");
    }

    let claude_resp: claude::Response = resp.json().await?;
    let text: String = claude_resp
        .content
        .iter()
        .map(|claude::ResponseBlock::Text { text }| text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let u = claude_resp.usage.unwrap_or_default();

    Ok(PageResult {
        text,
        skipped: false,
        input_tokens: u.input_tokens,
        output_tokens: u.output_tokens,
    })
}

async fn extract_title(
    client: &reqwest::Client,
    api_key: &str,
    text: &str,
) -> Result<(String, Usage)> {
    let snippet: String = text.chars().take(2000).collect();
    let result = claude::chat(
        client,
        api_key,
        "claude-sonnet-4-6",
        None,
        &format!(
            "What is the title of this document? Output only the title, nothing else.\n\n{}",
            snippet
        ),
        100,
        false,
    )
    .await?;
    Ok((result.text.trim().to_string(), result.usage))
}
