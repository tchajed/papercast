# TTS Podcast — Design Document

## Overview

A self-hosted web app that converts web articles, arXiv papers, and two-column PDFs into podcast episodes. Users submit URLs or PDFs via a web UI; the backend scrapes or extracts text, cleans it using Claude, synthesizes audio via TTS, generates a cover image, and publishes episodes to a private RSS feed consumable by any podcast client.

There is no user authentication. Access is controlled by secret feed tokens (UUIDs) embedded in RSS URLs and API requests.

## System Architecture

```
┌──────────────────────────────────────────────────┐
│              Fly.io (single app)                  │
│                                                   │
│  ┌────────────────────────────────────────────┐  │
│  │         Rust/Axum Backend                   │  │
│  │                                             │  │
│  │  Static Files ← SvelteKit build output     │  │
│  │  API Routes   ← Feed CRUD, episodes, RSS   │  │
│  │  Worker       ← Background job processing  │  │
│  └─────────┬──────────┬──────────┬────────────┘  │
│            │          │          │                │
│  ┌─────────▼──┐  ┌───▼────┐  ┌─▼─────────┐     │
│  │  SQLite    │  │ Claude │  │  Tigris    │     │
│  │  (volume)  │  │  API   │  │  (S3)     │     │
│  │  ↕         │  └───┬────┘  │  - audio  │     │
│  │ Litestream─┼──────┼───────▶  - images │     │
│  │  backup    │      │       │  - backup │     │
│  └────────────┘  ┌───▼──────┐└───────────┘     │
│                  │ TTS APIs │                    │
│                  │ OpenAI   │                    │
│                  │ ElevenL. │                    │
│                  │ Google   │                    │
│                  └──────────┘                    │
└──────────────────────────────────────────────────┘
```

## Data Model

### Feeds

Each feed is a podcast channel. Identified by a secret `feed_token` (UUID as TEXT).

| Column | Type | Purpose |
|--------|------|---------|
| id | TEXT (UUID) | Primary key |
| slug | TEXT | Human-readable ("ml-papers") |
| title | TEXT | Display name |
| feed_token | TEXT (UUID) | Secret token for RSS URL and API access |
| tts_default | TEXT | Default TTS provider (openai/elevenlabs/google) |

### Episodes

| Column | Type | Purpose |
|--------|------|---------|
| id | TEXT (UUID) | Primary key |
| feed_id | TEXT | Parent feed |
| title | TEXT | Article/paper title |
| source_url | TEXT | Original URL (null for PDF uploads) |
| source_type | TEXT | "article", "arxiv", or "pdf" |
| raw_text | TEXT | Content after scraping/extraction |
| cleaned_text | TEXT | Content after Claude cleanup |
| audio_url | TEXT | Public URL to MP3 on Tigris |
| image_url | TEXT | Public URL to cover image (nullable) |
| duration_secs | INTEGER | Exact MP3 duration |
| status | TEXT | pending → scraping → cleaning → tts → done/error |

### Jobs

| Column | Type | Purpose |
|--------|------|---------|
| episode_id | TEXT | Which episode |
| job_type | TEXT | scrape, pdf, clean, tts, or image |
| status | TEXT | queued → running → done/error |
| attempts | INTEGER | Retry count |
| run_after | TEXT | ISO8601 datetime for delayed execution |

### SQLite Configuration

WAL mode is enabled for concurrent reads during writes. The worker runs jobs inline (not spawned) to avoid concurrent SQLite writes.

## API

Base path: `/api/v1`. Admin routes require `Authorization: Bearer {ADMIN_TOKEN}`.

### Feeds

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | /api/v1/feeds | Admin | Create a feed |
| GET | /api/v1/feeds | Admin | List all feeds |
| GET | /api/v1/feeds/:token | Token | Get feed + episodes |
| DELETE | /api/v1/feeds/:token | Admin | Delete a feed |

### Episodes

| Method | Path | Auth | Description |
|--------|------|------|-------------|
| POST | /api/v1/feeds/:token/episodes | Token | Submit URL |
| POST | /api/v1/feeds/:token/episodes/pdf | Token | Upload PDF (multipart) |
| GET | /api/v1/feeds/:token/episodes/:id | Token | Get episode |
| DELETE | /api/v1/feeds/:token/episodes/:id | Token | Delete episode |
| POST | /api/v1/feeds/:token/episodes/:id/retry | Token | Retry failed episode |

### RSS

| Method | Path | Description |
|--------|------|-------------|
| GET | /feed/:token/rss.xml | RSS 2.0 with iTunes extensions and per-episode images |

## Processing Pipeline

```
URL:  submitted → [scrape] → [clean] → [tts] → done → [image]
PDF:  submitted → [pdf]    → [clean] → [tts] → done → [image]
```

### Scrape (articles and arXiv)

- **Articles**: HTTP GET → readability extraction
- **arXiv**: Metadata from arXiv API, HTML from ar5iv.org (LaTeX→HTML)

### PDF Extraction

PDF bytes sent to Claude vision as a document. Claude extracts text in reading order, handling two-column layouts. A separate Claude call extracts the title if not provided at upload.

### Clean (Claude)

Source-type-specific prompts:
- **Articles** (Sonnet): Remove non-article content, fix encoding
- **arXiv/PDF** (Opus): Remove citations, rewrite equations as spoken English, remove LaTeX artifacts

### TTS

Three providers:
- **OpenAI** (`tts-1-hd`, configurable voice)
- **ElevenLabs** (`eleven_flash_v2_5`, configurable voice ID)
- **Google Cloud TTS** (Journey voices, configurable)

Text split into ~4000-char chunks at sentence boundaries. Chunks processed sequentially, MP3 frames concatenated. Exact duration via `mp3-duration` crate.

### Cover Image (Gemini)

Runs after episode reaches `done` state. Claude generates a 2-sentence summary, Gemini generates an illustration. Failures are non-fatal — episodes are published before the image is ready.

### Error Handling

Failed jobs retry with exponential backoff (2min, 4min, 8min). After 3 attempts, the episode enters error state. Image failures are silently logged.

## Frontend

SvelteKit SPA (adapter-static), served by Axum as static files.

Three pages:
1. **Feed list** (/) — Admin view with create/delete
2. **Feed view** (/feeds/[token]) — URL submission, PDF upload, episode list with status polling
3. **Episode detail** (/feeds/[token]/episodes/[id]) — Full info, audio player, cover image, retry

## Deployment

Single Fly.io app. SQLite on a Fly volume, backed up to Tigris via Litestream. No Postgres, no Vercel. See [DEPLOYMENT.md](DEPLOYMENT.md).

## Configuration

All via environment variables. See `.env.example`.

Required: `DATABASE_URL`, `ANTHROPIC_API_KEY`, `ADMIN_TOKEN`, S3/Tigris credentials, at least one TTS provider key.

Optional: `GOOGLE_API_KEY` (for Google TTS and Gemini images), `GENERATE_IMAGES` (default true).
