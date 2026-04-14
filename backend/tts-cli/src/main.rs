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
        #[arg(long, default_value = "en-US-Journey-D")]
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

        /// Output MP3 file path
        #[arg(short, long, default_value = "output.mp3")]
        output: String,

        /// TTS voice name
        #[arg(long, default_value = "en-US-Journey-D")]
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
    match extractor {
        "claude" => tts_lib::pdf::extract(pdf_path, &anthropic_key()?).await,
        "gemini" => tts_lib::pdf_gemini::extract(pdf_path, &google_studio_key()?).await,
        other => anyhow::bail!("Unknown extractor: {other} (expected claude or gemini)"),
    }
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
            let doc = tts_lib::clean::clean(&doc, &provider).await?;
            print_document(&doc)?;
        }

        Command::Summarize { provider } => {
            let doc = read_stdin_document()?;
            let provider = make_provider(&provider)?;
            let doc = tts_lib::summarize::summarize(&doc, &provider).await?;
            print_document(&doc)?;
        }

        Command::Describe { provider } => {
            let doc = read_stdin_document()?;
            let provider = make_provider(&provider)?;
            let description = tts_lib::describe::describe(&doc, &provider).await?;
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
            let summary = tts_lib::image::visual_summary(text, &provider).await?;
            eprintln!("Visual brief: {summary}");
            let image = tts_lib::image::generate_image(&google_studio_key()?, &summary).await?;
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
            let result = tts_lib::tts::synthesize(text, &tts_config, None).await?;

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
            doc = tts_lib::clean::clean(&doc, &provider).await?;
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
                doc = tts_lib::summarize::summarize(&doc, &provider).await?;
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
            let result = tts_lib::tts::synthesize(text, &tts_config, None).await?;

            tokio::fs::write(&output, &result.audio).await?;
            eprintln!(
                "Wrote {} ({} chunks, {}s)",
                output, result.chunks_total, result.duration_secs
            );
        }
    }

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
