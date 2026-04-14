use axum::{
    extract::{Path, State},
    http::header,
    response::IntoResponse,
    routing::get,
    Router,
};
use sqlx::FromRow;
use time::{
    format_description::well_known::Rfc2822, PrimitiveDateTime, UtcOffset,
};

use crate::error::{AppError, AppResult};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/feed/{feed_token}/rss.xml", get(rss_feed))
}

#[derive(FromRow)]
struct FeedInfo {
    id: String,
    title: String,
    description: String,
    feed_token: String,
    image_url: Option<String>,
}

#[derive(FromRow)]
struct RssEpisode {
    id: String,
    title: String,
    source_url: Option<String>,
    audio_url: Option<String>,
    image_url: Option<String>,
    duration_secs: Option<i32>,
    pub_date: Option<String>,
    audio_bytes: Option<i64>,
    description: Option<String>,
    sections_json: Option<String>,
    summarize: i32,
}

async fn rss_feed(
    State(state): State<AppState>,
    Path(feed_token): Path<String>,
) -> AppResult<impl IntoResponse> {
    let feed = sqlx::query_as::<_, FeedInfo>(
        "SELECT id, title, description, feed_token, image_url FROM feeds WHERE feed_token = $1",
    )
    .bind(&feed_token)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let episodes = sqlx::query_as::<_, RssEpisode>(
        "SELECT id, title, source_url, audio_url, image_url, duration_secs, pub_date, audio_bytes, description, sections_json, summarize
         FROM episodes
         WHERE feed_id = $1 AND status = 'done' AND audio_url IS NOT NULL
         ORDER BY pub_date DESC
         LIMIT 50",
    )
    .bind(&feed.id)
    .fetch_all(&state.pool)
    .await?;

    let feed_link = format!(
        "{}/feeds/{}",
        state.config.public_url, feed.feed_token
    );
    let rss_link = format!(
        "{}/feed/{}/rss.xml",
        state.config.public_url, feed.feed_token
    );

    let mut items = String::new();
    for ep in &episodes {
        let pub_date = ep
            .pub_date
            .as_deref()
            .and_then(format_rfc2822)
            .unwrap_or_default();
        let duration = ep.duration_secs.unwrap_or(0);
        let audio_url = ep.audio_url.as_deref().unwrap_or("");
        let length = ep.audio_bytes.unwrap_or(0);
        let base_description = ep
            .description
            .as_deref()
            .unwrap_or_else(|| ep.source_url.as_deref().unwrap_or("PDF upload"));
        let mut description = match ep.source_url.as_deref() {
            Some(url) if !base_description.contains(url) => {
                format!("{base_description}\n\nSource: {url}")
            }
            _ => base_description.to_string(),
        };
        if let Some(chapters) = render_chapters(ep.sections_json.as_deref()) {
            description.push_str("\n\nChapters:\n");
            description.push_str(&chapters);
        }

        let image_tag = if let Some(ref img_url) = ep.image_url {
            format!(
                "\n      <itunes:image href=\"{}\"/>",
                xml_escape(img_url)
            )
        } else {
            String::new()
        };

        items.push_str(&format!(
            r#"    <item>
      <title>{title}</title>
      <guid isPermaLink="false">{id}</guid>
      <pubDate>{pub_date}</pubDate>
      <description>{description}</description>
      <enclosure url="{audio_url}" length="{length}" type="audio/mpeg"/>
      <itunes:duration>{duration}</itunes:duration>{image_tag}
    </item>
"#,
            title = xml_escape(&display_title(&ep.title, ep.summarize)),
            id = ep.id,
            pub_date = pub_date,
            description = xml_escape(&description),
            audio_url = xml_escape(audio_url),
            length = length,
            duration = duration,
            image_tag = image_tag,
        ));
    }

    let channel_image_tag = feed
        .image_url
        .as_deref()
        .map(|img_url| {
            format!(
                "\n    <itunes:image href=\"{url}\"/>\n    <image>\n      <url>{url}</url>\n      <title>{title}</title>\n      <link>{link}</link>\n    </image>",
                url = xml_escape(img_url),
                title = xml_escape(&feed.title),
                link = xml_escape(&feed_link),
            )
        })
        .unwrap_or_default();

    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:itunes="http://www.itunes.com/dtds/podcast-1.0.dtd" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>{title}</title>
    <description>{description}</description>
    <link>{link}</link>
    <atom:link href="{rss_link}" rel="self" type="application/rss+xml"/>
    <language>en-us</language>
    <itunes:author>Personal Podcast</itunes:author>
    <itunes:category text="Technology"/>
    <itunes:explicit>false</itunes:explicit>{channel_image_tag}
{items}  </channel>
</rss>"#,
        title = xml_escape(&feed.title),
        description = xml_escape(&feed.description),
        link = xml_escape(&feed_link),
        rss_link = xml_escape(&rss_link),
        channel_image_tag = channel_image_tag,
        items = items,
    );

    Ok((
        [
            (
                header::CONTENT_TYPE,
                "application/rss+xml; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "max-age=300"),
        ],
        xml,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape_ampersand() {
        assert_eq!(xml_escape("A & B"), "A &amp; B");
    }

    #[test]
    fn test_xml_escape_angle_brackets() {
        assert_eq!(xml_escape("<tag>"), "&lt;tag&gt;");
    }

    #[test]
    fn test_xml_escape_quotes() {
        assert_eq!(xml_escape(r#"say "hello""#), "say &quot;hello&quot;");
    }

    #[test]
    fn test_xml_escape_apostrophe() {
        assert_eq!(xml_escape("it's"), "it&apos;s");
    }

    #[test]
    fn test_xml_escape_all_special() {
        assert_eq!(
            xml_escape(r#"<a href="x">&'y'"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;&apos;y&apos;"
        );
    }

    #[test]
    fn test_xml_escape_no_special() {
        assert_eq!(xml_escape("plain text"), "plain text");
    }

    #[test]
    fn test_xml_escape_empty() {
        assert_eq!(xml_escape(""), "");
    }

    #[test]
    fn test_format_rfc2822_sqlite() {
        assert_eq!(
            format_rfc2822("2026-04-14 14:59:47").as_deref(),
            Some("Tue, 14 Apr 2026 14:59:47 +0000"),
        );
    }

    #[test]
    fn test_format_rfc2822_invalid() {
        assert_eq!(format_rfc2822("not a date"), None);
        assert_eq!(format_rfc2822(""), None);
    }
}

/// Convert SQLite's `YYYY-MM-DD HH:MM:SS` (UTC) into RFC 2822 for RSS pubDate.
/// Returns None if the input doesn't parse; callers should fall back to empty.
fn display_title(title: &str, summarize: i32) -> String {
    if summarize != 0 {
        format!("{title} (Summary)")
    } else {
        title.to_string()
    }
}

fn format_rfc2822(s: &str) -> Option<String> {
    let fmt = time::format_description::parse(
        "[year]-[month]-[day] [hour]:[minute]:[second]",
    )
    .ok()?;
    let primitive = PrimitiveDateTime::parse(s, &fmt).ok()?;
    primitive.assume_offset(UtcOffset::UTC).format(&Rfc2822).ok()
}

/// Render stored sections JSON into a human-readable chapter list, one line
/// per section prefixed with `HH:MM:SS` (or `MM:SS` for short episodes).
/// Returns None if the JSON is absent/empty/malformed.
fn render_chapters(sections_json: Option<&str>) -> Option<String> {
    let json = sections_json?;
    let sections: Vec<tts_lib::tts::Section> = serde_json::from_str(json).ok()?;
    if sections.is_empty() {
        return None;
    }
    let use_hours = sections.last().map(|s| s.start_secs >= 3600.0).unwrap_or(false);
    let mut out = String::new();
    for s in &sections {
        out.push_str(&format_timestamp(s.start_secs, use_hours));
        out.push(' ');
        out.push_str(&s.title);
        out.push('\n');
    }
    Some(out)
}

fn format_timestamp(secs: f64, use_hours: bool) -> String {
    let total = secs.max(0.0) as u64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if use_hours {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
