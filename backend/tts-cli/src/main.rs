use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::io::Read;

#[derive(Parser)]
#[command(name = "tts-cli", about = "TTS podcast pipeline CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Extract text from a PDF file using Claude vision or Gemini.
    /// Outputs JSON document with title and raw_text.
    ExtractPdf {
        /// Path to PDF file
        pdf_path: String,

        /// Extractor: claude (page-by-page vision) or gemini (single inline request)
        #[arg(long, default_value = "claude")]
        extractor: String,
    },

    /// Scrape a URL and extract readable text.
    /// Outputs JSON document with title and raw_text.
    Scrape {
        /// URL to scrape
        url: String,

        /// Source type: article or arxiv
        #[arg(long, default_value = "article")]
        source_type: String,
    },

    /// Clean raw text for TTS.
    /// Reads JSON document from stdin (needs raw_text and source_type).
    /// Outputs JSON document with cleaned_text added.
    Clean {
        /// Provider: claude or gemini
        #[arg(long, default_value = "claude")]
        provider: String,
    },

    /// Summarize cleaned text into a podcast-style transcript.
    /// Reads JSON document from stdin (needs cleaned_text).
    /// Outputs JSON document with transcript added.
    Summarize {
        /// Provider: claude or gemini
        #[arg(long, default_value = "claude")]
        provider: String,

        /// Optional focus: narrow the summary to this topic / angle.
        #[arg(long)]
        focus: Option<String>,
    },

    /// Generate a short episode description from cleaned_text or transcript.
    /// Reads JSON document from stdin; prints the description to stdout.
    Describe {
        /// Provider: claude or gemini
        #[arg(long, default_value = "claude")]
        provider: String,
    },

    /// Generate a cover image and write it to --output.
    /// Reads JSON document from stdin (uses cleaned_text or transcript for the brief).
    Image {
        /// Output image file path (extension is chosen by mime type)
        #[arg(short, long, default_value = "cover")]
        output: String,

        /// Provider to use for the visual summary step (claude or gemini)
        #[arg(long, default_value = "claude")]
        provider: String,
    },

    /// Synthesize text to MP3 audio using Google Cloud TTS.
    /// Reads JSON document from stdin (needs cleaned_text or transcript).
    /// Writes MP3 to --output file.
    Tts {
        /// Output MP3 file path
        #[arg(short, long, default_value = "output.mp3")]
        output: String,

        /// TTS voice name
        #[arg(long, default_value = "en-US-Neural2-D")]
        voice: String,
    },

    /// Run the full pipeline: extract/scrape → clean → [summarize] → tts.
    Pipeline {
        /// PDF file path or URL to process
        source: String,

        /// Source type: article, arxiv, or pdf (auto-detected if omitted)
        #[arg(long)]
        source_type: Option<String>,

        /// Summarize before TTS
        #[arg(long)]
        summarize: bool,

        /// Optional summarization focus
        #[arg(long)]
        focus: Option<String>,

        /// Output MP3 file path
        #[arg(short, long, default_value = "output.mp3")]
        output: String,

        /// TTS voice name
        #[arg(long, default_value = "en-US-Neural2-D")]
        voice: String,

        /// Stop after this stage (extract, clean, summarize) and print JSON
        #[arg(long)]
        stop_after: Option<String>,

        /// PDF extractor: claude or gemini
        #[arg(long, default_value = "claude")]
        extractor: String,

        /// Provider for clean/summarize: claude or gemini
        #[arg(long, default_value = "claude")]
        provider: String,
    },

    /// Query the admin /usage endpoint on a deployed server and print a cost
    /// breakdown by provider/model/stage. Requires SERVER_URL and ADMIN_TOKEN
    /// env vars (or --server / --token flags).
    Costs {
        /// Server base URL (e.g. https://podcast.example.com).
        #[arg(long, env = "SERVER_URL")]
        server: String,

        /// Admin bearer token.
        #[arg(long, env = "ADMIN_TOKEN")]
        token: String,

        /// Optional window in days (default: all time).
        #[arg(long)]
        days: Option<i64>,

        /// Episode ID: when set, show per-episode breakdown instead of summary.
        #[arg(long)]
        episode: Option<String>,
    },
}

fn make_provider(name: &str) -> Result<tts_lib::Provider> {
    match name {
        "claude" => Ok(tts_lib::Provider::claude(anthropic_key()?)),
        "gemini" => Ok(tts_lib::Provider::gemini_default(google_studio_key()?)),
        other => anyhow::bail!("Unknown provider: {other} (expected claude or gemini)"),
    }
}

fn read_stdin_document() -> Result<tts_lib::Document> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    serde_json::from_str(&input).context("Failed to parse JSON from stdin")
}

fn print_document(doc: &tts_lib::Document) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(doc)?);
    Ok(())
}

fn anthropic_key() -> Result<String> {
    std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY not set")
}

fn google_tts_key() -> Result<String> {
    std::env::var("GOOGLE_TTS_API_KEY").context("GOOGLE_TTS_API_KEY not set")
}

fn google_studio_key() -> Result<String> {
    std::env::var("GOOGLE_STUDIO_API_KEY").context("GOOGLE_STUDIO_API_KEY not set")
}

async fn extract_pdf(pdf_path: &str, extractor: &str) -> Result<tts_lib::Document> {
    let (doc, usages) = match extractor {
        "claude" => tts_lib::pdf::extract(pdf_path, &anthropic_key()?).await?,
        "gemini" => tts_lib::pdf_gemini::extract(pdf_path, &google_studio_key()?).await?,
        other => anyhow::bail!("Unknown extractor: {other} (expected claude or gemini)"),
    };
    for u in &usages {
        eprintln!(
            "usage: {} {} input={} output={}",
            u.provider, u.model, u.input_tokens, u.output_tokens
        );
    }
    Ok(doc)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::ExtractPdf { pdf_path, extractor } => {
            let doc = extract_pdf(&pdf_path, &extractor).await?;
            print_document(&doc)?;
        }

        Command::Scrape { url, source_type } => {
            let doc = tts_lib::scrape::scrape(&url, &source_type).await?;
            print_document(&doc)?;
        }

        Command::Clean { provider } => {
            let doc = read_stdin_document()?;
            let provider = make_provider(&provider)?;
            let (doc, usages) = tts_lib::clean::clean(&doc, &provider).await?;
            for usage in &usages {
                eprintln!("usage: {} {} input={} output={}", usage.provider, usage.model, usage.input_tokens, usage.output_tokens);
            }
            print_document(&doc)?;
        }

        Command::Summarize { provider, focus } => {
            let doc = read_stdin_document()?;
            let provider = make_provider(&provider)?;
            let (doc, usage) = tts_lib::summarize::summarize(&doc, &provider, focus.as_deref()).await?;
            eprintln!("usage: {} {} input={} output={}", usage.provider, usage.model, usage.input_tokens, usage.output_tokens);
            print_document(&doc)?;
        }

        Command::Describe { provider } => {
            let doc = read_stdin_document()?;
            let provider = make_provider(&provider)?;
            let (description, usage) = tts_lib::describe::describe(&doc, &provider).await?;
            eprintln!("usage: {} {} input={} output={}", usage.provider, usage.model, usage.input_tokens, usage.output_tokens);
            println!("{description}");
        }

        Command::Image { output, provider } => {
            let doc = read_stdin_document()?;
            let text = doc
                .cleaned_text
                .as_deref()
                .or(doc.transcript.as_deref())
                .context("No cleaned_text or transcript in input")?;
            let provider = make_provider(&provider)?;
            let (summary, summary_usage) = tts_lib::image::visual_summary(text, &provider).await?;
            eprintln!("Visual brief: {summary}");
            eprintln!("usage: {} {} input={} output={}", summary_usage.provider, summary_usage.model, summary_usage.input_tokens, summary_usage.output_tokens);
            let (image, image_usage) = tts_lib::image::generate_image(&google_studio_key()?, &summary).await?;
            eprintln!("usage: {} {} input={} output={}", image_usage.provider, image_usage.model, image_usage.input_tokens, image_usage.output_tokens);
            let ext = match image.mime_type.as_str() {
                "image/png" => "png",
                "image/jpeg" => "jpg",
                "image/webp" => "webp",
                _ => "bin",
            };
            let path = format!("{output}.{ext}");
            tokio::fs::write(&path, &image.bytes).await?;
            eprintln!("Wrote {path} ({} bytes, {})", image.bytes.len(), image.mime_type);
        }

        Command::Tts { output, voice } => {
            let doc = read_stdin_document()?;
            let text = doc
                .tts_text()
                .context("No cleaned_text or transcript in input")?;

            let tts_config = tts_lib::tts::TtsConfig::new(google_tts_key()?).with_voice(voice);
            let result = tts_lib::tts::synthesize(text, &tts_config, None, None).await?;

            tokio::fs::write(&output, &result.audio).await?;
            eprintln!(
                "Wrote {} ({} chunks, {}s)",
                output, result.chunks_total, result.duration_secs
            );
        }

        Command::Pipeline {
            source,
            source_type,
            summarize,
            focus,
            output,
            voice,
            stop_after,
            extractor,
            provider,
        } => {
            let source_type = source_type.unwrap_or_else(|| detect_source_type(&source));
            let provider = make_provider(&provider)?;

            // Stage 1: Extract
            eprintln!("--- Extract ({source_type}, {extractor}) ---");
            let mut doc = if source_type == "pdf" {
                extract_pdf(&source, &extractor).await?
            } else {
                tts_lib::scrape::scrape(&source, &source_type).await?
            };

            eprintln!(
                "Title: {}",
                doc.title.as_deref().unwrap_or("(none)")
            );
            eprintln!(
                "Raw text: {} chars",
                doc.raw_text.as_ref().map_or(0, |t| t.len())
            );

            if stop_after.as_deref() == Some("extract") {
                print_document(&doc)?;
                return Ok(());
            }

            // Stage 2: Clean
            eprintln!("--- Clean ---");
            let (new_doc, _clean_usages) = tts_lib::clean::clean(&doc, &provider).await?;
            doc = new_doc;
            eprintln!(
                "Cleaned text: {} words",
                doc.word_count.unwrap_or(0)
            );

            if stop_after.as_deref() == Some("clean") {
                print_document(&doc)?;
                return Ok(());
            }

            // Stage 3: Summarize (optional)
            if summarize {
                eprintln!("--- Summarize ---");
                let (new_doc, _sum_usage) = tts_lib::summarize::summarize(&doc, &provider, focus.as_deref()).await?;
                doc = new_doc;
                eprintln!(
                    "Transcript: {} words",
                    doc.word_count.unwrap_or(0)
                );

                if stop_after.as_deref() == Some("summarize") {
                    print_document(&doc)?;
                    return Ok(());
                }
            }

            // Stage 4: TTS
            eprintln!("--- TTS ---");
            let text = doc.tts_text().context("No text for TTS")?;
            let tts_config = tts_lib::tts::TtsConfig::new(google_tts_key()?).with_voice(voice);
            let result = tts_lib::tts::synthesize(text, &tts_config, None, None).await?;

            tokio::fs::write(&output, &result.audio).await?;
            eprintln!(
                "Wrote {} ({} chunks, {}s)",
                output, result.chunks_total, result.duration_secs
            );
        }

        Command::Costs { server, token, days, episode } => {
            run_costs(&server, &token, days, episode.as_deref()).await?;
        }
    }

    Ok(())
}

async fn run_costs(
    server: &str,
    token: &str,
    days: Option<i64>,
    episode: Option<&str>,
) -> Result<()> {
    let server = server.trim_end_matches('/');
    let client = reqwest::Client::new();

    if let Some(ep) = episode {
        #[derive(serde::Deserialize)]
        struct Row {
            stage: String,
            provider: String,
            model: String,
            input_tokens: i64,
            output_tokens: i64,
            created_at: String,
        }
        #[derive(serde::Deserialize)]
        struct Resp {
            rows: Vec<Row>,
            estimated_usd: f64,
        }
        let url = format!("{server}/api/v1/admin/usage/episode/{ep}");
        let resp: Resp = client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        println!("{:<14} {:<10} {:<28} {:>10} {:>10}  {}", "stage", "provider", "model", "input", "output", "when");
        for r in &resp.rows {
            println!(
                "{:<14} {:<10} {:<28} {:>10} {:>10}  {}",
                r.stage, r.provider, r.model, r.input_tokens, r.output_tokens, r.created_at
            );
        }
        println!("\nEstimated total: ${:.4}", resp.estimated_usd);
        return Ok(());
    }

    #[derive(serde::Deserialize)]
    struct Group {
        provider: String,
        model: String,
        stage: String,
        calls: i64,
        input_tokens: i64,
        output_tokens: i64,
    }
    #[derive(serde::Deserialize)]
    struct Resp {
        since: Option<String>,
        groups: Vec<Group>,
        total_estimated_usd: f64,
    }

    let mut url = format!("{server}/api/v1/admin/usage");
    if let Some(d) = days {
        url.push_str(&format!("?days={d}"));
    }
    let resp: Resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    match &resp.since {
        Some(s) => println!("Usage over last {s}:"),
        None => println!("Usage (all time):"),
    }
    println!(
        "{:<12} {:<28} {:<14} {:>6} {:>12} {:>12}",
        "provider", "model", "stage", "calls", "input", "output"
    );
    for g in &resp.groups {
        println!(
            "{:<12} {:<28} {:<14} {:>6} {:>12} {:>12}",
            g.provider, g.model, g.stage, g.calls, g.input_tokens, g.output_tokens
        );
    }
    println!("\nEstimated total: ${:.4}", resp.total_estimated_usd);
    Ok(())
}

fn detect_source_type(source: &str) -> String {
    if source.ends_with(".pdf") || std::path::Path::new(source).exists() {
        "pdf".to_string()
    } else if source.contains("arxiv.org") || source.contains("ar5iv.org") {
        "arxiv".to_string()
    } else {
        "article".to_string()
    }
}
