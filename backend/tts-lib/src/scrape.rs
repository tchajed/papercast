use anyhow::{Context, Result};
use reqwest::Client;
use url::Url;

use crate::Document;

/// Scrape a URL and extract readable text.
/// Handles both regular articles and arXiv papers.
pub async fn scrape(source_url: &str, source_type: &str) -> Result<Document> {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let (title, raw_text) = match source_type {
        "arxiv" => scrape_arxiv(&client, source_url).await?,
        _ => scrape_article(&client, source_url).await?,
    };

    Ok(Document {
        title: Some(title),
        source_type: source_type.to_string(),
        raw_text: Some(raw_text),
        ..Default::default()
    })
}

fn extract_readable(html: &str, url_str: &str) -> Result<(String, String)> {
    let url = Url::parse(url_str).context("Invalid URL")?;
    let mut cursor = std::io::Cursor::new(html.as_bytes());
    let product = readability::extractor::extract(&mut cursor, &url)
        .map_err(|e| anyhow::anyhow!("Readability extraction failed: {:?}", e))?;

    let title = if product.title.is_empty() {
        url_str.to_string()
    } else {
        product.title
    };

    Ok((title, product.text))
}

async fn scrape_article(_client: &Client, url: &str) -> Result<(String, String)> {
    match fetch_article(url).await? {
        ArticleFetch::Html { title, text } => Ok((title, text)),
        ArticleFetch::Pdf(_) => {
            anyhow::bail!("URL returned a PDF, not HTML: {url}")
        }
    }
}

/// Result of fetching a URL that was expected to be an article — either
/// readable HTML or a PDF body that the caller should run through PDF
/// extraction. Some sites (e.g. journals) serve PDFs from URLs that look like
/// regular article pages, so we detect by Content-Type rather than relying on
/// the URL extension.
pub enum ArticleFetch {
    Html { title: String, text: String },
    Pdf(Vec<u8>),
}

pub async fn fetch_article(url: &str) -> Result<ArticleFetch> {
    let client = Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let resp = client.get(url).send().await?.error_for_status()?;

    let is_pdf = url_looks_like_pdf(resp.url().as_str())
        || resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| {
                ct.trim()
                    .to_ascii_lowercase()
                    .starts_with("application/pdf")
            })
            .unwrap_or(false);

    if is_pdf {
        let bytes = resp.bytes().await?.to_vec();
        return Ok(ArticleFetch::Pdf(bytes));
    }

    let html = resp.text().await?;
    let (title, text) = extract_readable(&html, url)?;
    Ok(ArticleFetch::Html { title, text })
}

pub fn url_looks_like_pdf(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.to_ascii_lowercase().ends_with(".pdf")
}

async fn scrape_arxiv(client: &Client, url: &str) -> Result<(String, String)> {
    let arxiv_id = extract_arxiv_id(url).context("Could not extract arXiv ID from URL")?;

    // Fetch the LaTeXML-rendered HTML. Prefer arxiv.org/html (canonical) and
    // fall back to ar5iv.org if arxiv hasn't rendered or returns an error —
    // very old papers don't have a LaTeXML build on arxiv.org yet.
    let (html, fetch_url) = fetch_arxiv_html(client, &arxiv_id).await?;

    // Both sources emit `<article class="ltx_document">` with the full paper.
    // readability's content-density heuristic discards the main body on many
    // LaTeXML papers (it scores the longest continuous block — often the
    // appendix — highest), so walk the LaTeXML tree directly.
    let (title, text) = extract_latexml(&html)
        .unwrap_or_else(|| extract_readable(&html, &fetch_url).unwrap_or_default());

    if text.trim().len() < 500 {
        anyhow::bail!(
            "{fetch_url} returned suspiciously short content ({} chars) for {arxiv_id}",
            text.trim().len()
        );
    }

    // Prefer the arXiv API title if available — it's cleaner than the HTML one.
    let title = match fetch_arxiv_title(client, &arxiv_id).await {
        Ok(Some(t)) => t,
        _ => {
            if title.is_empty() {
                format!("arXiv:{arxiv_id}")
            } else {
                title
            }
        }
    };

    Ok((title, text))
}

async fn fetch_arxiv_html(client: &Client, arxiv_id: &str) -> Result<(String, String)> {
    let arxiv_url = format!("https://arxiv.org/html/{arxiv_id}");
    match client.get(&arxiv_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            let html = resp.text().await?;
            if !html.contains("Conversion to HTML had a Fatal error") {
                return Ok((html, arxiv_url));
            }
            tracing::info!("arxiv.org/html failed for {arxiv_id}, falling back to ar5iv");
        }
        Ok(resp) => {
            tracing::info!(
                "arxiv.org/html returned {} for {arxiv_id}, falling back to ar5iv",
                resp.status()
            );
        }
        Err(e) => {
            tracing::info!(
                "arxiv.org/html request failed for {arxiv_id}: {e}, falling back to ar5iv"
            );
        }
    }

    let ar5iv_url = format!("https://ar5iv.org/abs/{arxiv_id}");
    let html_resp = client.get(&ar5iv_url).send().await?.error_for_status()?;
    let html = html_resp.text().await?;
    if html.contains("Conversion to HTML had a Fatal error") {
        anyhow::bail!("ar5iv conversion failed for {arxiv_id}");
    }
    Ok((html, ar5iv_url))
}

async fn fetch_arxiv_title(client: &Client, arxiv_id: &str) -> Result<Option<String>> {
    let api_url = format!("https://export.arxiv.org/api/query?id_list={arxiv_id}");
    let resp = client.get(&api_url).send().await?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let xml = resp.text().await?;
    Ok(parse_arxiv_title(&xml))
}

/// Extract title + text from LaTeXML-generated HTML (arxiv.org/html, ar5iv).
/// Returns None if the document doesn't look like LaTeXML output.
fn extract_latexml(html: &str) -> Option<(String, String)> {
    let doc = scraper::Html::parse_document(html);
    let article_sel = scraper::Selector::parse("article.ltx_document").ok()?;
    let article = doc.select(&article_sel).next()?;

    let title_sel = scraper::Selector::parse("h1.ltx_title_document").ok()?;
    let title = doc
        .select(&title_sel)
        .next()
        .map(|e| normalize_whitespace(&collect_text(&e)))
        .unwrap_or_default();

    let mut out = String::new();
    render_latexml_node(*article, &mut out);
    let text = collapse_blank_lines(&out);
    Some((title, text))
}

fn collect_text(el: &scraper::ElementRef) -> String {
    el.text().collect::<String>()
}

fn normalize_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::new();
    let mut blank_run = 0;
    for line in s.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            blank_run += 1;
            if blank_run <= 1 {
                out.push('\n');
            }
        } else {
            blank_run = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// Walk a LaTeXML subtree, appending text to `out`. Skips citations,
/// bibliography, figures, and math (substituting `alttext` where present).
/// Inserts blank lines between block elements so the cleaner sees paragraph
/// structure.
fn render_latexml_node(node: ego_tree::NodeRef<scraper::node::Node>, out: &mut String) {
    use scraper::node::Node;

    match node.value() {
        Node::Text(t) => {
            let s: &str = t;
            // Collapse internal whitespace but don't trim — we rely on spacing
            // between adjacent inline nodes.
            for (i, token) in s.split_whitespace().enumerate() {
                if i > 0 || s.starts_with(char::is_whitespace) {
                    ensure_space(out);
                }
                out.push_str(token);
            }
            if s.ends_with(char::is_whitespace) {
                ensure_space(out);
            }
        }
        Node::Element(el) => {
            let name = el.name();
            let class = el.attr("class").unwrap_or("");

            // Skip noise: bibliography, references, citations, figures, nav/footer.
            if class.contains("ltx_bibliography")
                || class.contains("ltx_biblist")
                || class.contains("ltx_bibitem")
                || class.contains("ltx_ERROR")
                || class.contains("ltx_pagination")
                || class.contains("ltx_authors")
                || class.contains("ltx_dates")
                || class.contains("ltx_classification")
                || matches!(
                    name,
                    "cite" | "nav" | "footer" | "script" | "style" | "figure" | "table"
                )
            {
                return;
            }

            // Math: prefer the `alttext` attribute (a TeX-free rendering).
            if name == "math" {
                if let Some(alt) = el.attr("alttext") {
                    ensure_space(out);
                    out.push_str(alt);
                    ensure_space(out);
                }
                return;
            }

            let is_heading = matches!(name, "h1" | "h2" | "h3" | "h4" | "h5" | "h6");
            let is_block = is_heading
                || matches!(
                    name,
                    "p" | "section" | "article" | "div" | "li" | "blockquote"
                )
                || class.contains("ltx_para")
                || class.contains("ltx_paragraph")
                || class.contains("ltx_section")
                || class.contains("ltx_subsection")
                || class.contains("ltx_abstract");

            if is_block {
                ensure_blank_line(out);
            }

            for child in node.children() {
                render_latexml_node(child, out);
            }

            if is_block {
                ensure_blank_line(out);
            }
        }
        _ => {}
    }
}

fn ensure_space(out: &mut String) {
    if !out.is_empty() && !out.ends_with(char::is_whitespace) {
        out.push(' ');
    }
}

fn ensure_blank_line(out: &mut String) {
    // Trim trailing spaces and ensure we end with exactly two newlines
    // (one to end the current line, one blank separator) — but never leading.
    if out.is_empty() {
        return;
    }
    while out.ends_with(' ') {
        out.pop();
    }
    let nl_count = out.chars().rev().take_while(|&c| c == '\n').count();
    for _ in nl_count..2 {
        out.push('\n');
    }
}

pub fn extract_arxiv_id(url: &str) -> Option<String> {
    let patterns = [
        "arxiv.org/abs/",
        "arxiv.org/html/",
        "arxiv.org/pdf/",
        "ar5iv.org/abs/",
        "ar5iv.org/html/",
    ];
    for pat in patterns {
        if let Some(idx) = url.find(pat) {
            let rest = &url[idx + pat.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| *c != '/' && *c != '?')
                .collect();
            let id = id.strip_suffix(".pdf").unwrap_or(&id).to_string();
            if !id.is_empty() {
                return Some(id);
            }
        }
    }
    None
}

fn parse_arxiv_title(xml: &str) -> Option<String> {
    let entry_start = xml.find("<entry>")?;
    let after_entry = &xml[entry_start..];
    let title_start = after_entry.find("<title>")? + 7;
    let title_end = after_entry[title_start..].find("</title>")?;
    let title = &after_entry[title_start..title_start + title_end];
    Some(title.trim().replace('\n', " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_arxiv_id_standard() {
        assert_eq!(
            extract_arxiv_id("https://arxiv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_ar5iv() {
        assert_eq!(
            extract_arxiv_id("https://ar5iv.org/abs/2301.12345"),
            Some("2301.12345".into())
        );
    }

    #[test]
    fn test_extract_arxiv_id_none() {
        assert_eq!(extract_arxiv_id("https://example.com"), None);
    }

    #[test]
    fn test_parse_arxiv_title_valid() {
        let xml = r#"<?xml version="1.0"?>
<feed><entry><title>Attention Is All You Need</title></entry></feed>"#;
        assert_eq!(
            parse_arxiv_title(xml),
            Some("Attention Is All You Need".into())
        );
    }
}
