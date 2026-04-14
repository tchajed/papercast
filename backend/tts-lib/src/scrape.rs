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

async fn scrape_article(client: &Client, url: &str) -> Result<(String, String)> {
    let resp = client.get(url).send().await?.error_for_status()?;
    let html = resp.text().await?;
    extract_readable(&html, url)
}

async fn scrape_arxiv(client: &Client, url: &str) -> Result<(String, String)> {
    let arxiv_id = extract_arxiv_id(url).context("Could not extract arXiv ID from URL")?;

    let api_url = format!("https://export.arxiv.org/api/query?id_list={arxiv_id}");
    let api_resp = client.get(&api_url).send().await?.error_for_status()?;
    let api_xml = api_resp.text().await?;

    let title = parse_arxiv_title(&api_xml).unwrap_or_else(|| format!("arXiv:{arxiv_id}"));

    let ar5iv_url = format!("https://ar5iv.org/abs/{arxiv_id}");
    let html_resp = client.get(&ar5iv_url).send().await?.error_for_status()?;
    let html = html_resp.text().await?;

    if html.contains("Conversion to HTML had a Fatal error") {
        anyhow::bail!("ar5iv conversion failed for {arxiv_id}");
    }

    let (_extracted_title, text) = extract_readable(&html, &ar5iv_url)?;

    if text.trim().len() < 500 {
        anyhow::bail!(
            "ar5iv returned suspiciously short content ({} chars) for {arxiv_id}",
            text.trim().len()
        );
    }

    Ok((title, text))
}

pub fn extract_arxiv_id(url: &str) -> Option<String> {
    let patterns = ["arxiv.org/abs/", "ar5iv.org/abs/"];
    for pat in patterns {
        if let Some(idx) = url.find(pat) {
            let rest = &url[idx + pat.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| *c != '/' && *c != '?')
                .collect();
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
