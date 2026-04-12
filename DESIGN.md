# TTS Podcast — Design Document

## Overview

A self-hosted web app that converts web articles and arXiv papers into podcast episodes. Users submit URLs via a web UI; the backend scrapes and cleans the text using Claude, synthesizes audio via TTS, and publishes episodes to a private RSS feed consumable by any podcast client.

There is no user authentication. Access is controlled by secret feed tokens (UUIDs) embedded in RSS URLs and API requests.

## System Architecture

```
┌──────────────┐     ┌─────────────────────────────────────┐
│   SvelteKit  │────>│         Rust/Axum Backend            │
│   Frontend   │     │                                      │
│  (Vercel)    │<────│  API Routes  │  Background Worker    │
└──────────────┘     │              │                       │
                     │              │  ┌─────────────────┐  │
                     │              │  │ scrape → clean → │  │
                     │              │  │ tts → upload     │  │
                     │              │  └─────────────────┘  │
                     └──────┬───────┬───────────┬───────────┘
                            │       │           │
                     ┌──────▼──┐ ┌──▼────┐ ┌────▼─────┐
                     │Postgres │ │Claude  │ │  Tigris  │
                     │ (Fly)   │ │  API   │ │   (S3)   │
                     └─────────┘ └───┬────┘ └──────────┘
                                     │
                              ┌──────▼──────┐
                              │ OpenAI /    │
                              │ ElevenLabs  │
                              │ TTS API     │
                              └─────────────┘
```

## Data Model

### Feeds

Each feed is a podcast channel with its own RSS URL. Feeds are identified by a secret `feed_token` (UUID) that serves as both an identifier and access credential.

| Column | Type | Purpose |
|--------|------|---------|
| id | UUID | Primary key |
| slug | TEXT | Human-readable identifier ("ml-papers") |
| title | TEXT | Display name |
| feed_token | UUID | Secret token for RSS URL and API access |
| tts_default | TEXT | Default TTS provider (openai/elevenlabs) |

### Episodes

Each episode represents a single article or paper being processed.

| Column | Type | Purpose |
|--------|------|---------|
| id | UUID | Primary key |
| feed_id | UUID | Parent feed |
| title | TEXT | Article/paper title (updated after scrape) |
| source_url | TEXT | Original URL |
| source_type | TEXT | "article" or "arxiv" |
| raw_text | TEXT | Content after scraping |
| cleaned_text | TEXT | Content after Claude cleanup |
| audio_url | TEXT | Public URL to MP3 on Tigris |
| status | TEXT | pending → scraping → cleaning → tts → done/error |

### Jobs

Background processing queue. One active job per episode at a time.

| Column | Type | Purpose |
|--------|------|---------|
| episode_id | UUID | Which episode this job processes |
| job_type | TEXT | scrape, clean, or tts |
| status | TEXT | queued → running → done/error |
| attempts | INTEGER | Retry count |
| run_after | TIMESTAMPTZ | Delayed execution (for backoff) |

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
| GET | /api/v1/feeds/:token/episodes/:id | Token | Get episode |
| DELETE | /api/v1/feeds/:token/episodes/:id | Token | Delete episode |
| POST | /api/v1/feeds/:token/episodes/:id/retry | Token | Retry failed episode |

### RSS

| Method | Path | Description |
|--------|------|-------------|
| GET | /feed/:token/rss.xml | RSS 2.0 feed with iTunes extensions |

## Processing Pipeline

### 1. Scrape

**Articles**: HTTP GET → readability extraction (strips nav, ads, footers).

**arXiv papers**: Fetches metadata from arXiv API, then HTML from ar5iv.org (LaTeX→HTML converter). Much better than PDF text extraction.

### 2. Clean (Claude)

Sends raw text to Claude with source-type-specific prompts:
- **Articles**: Remove non-article content, fix encoding
- **arXiv**: Remove citations, rewrite equations as spoken English, remove LaTeX artifacts

### 3. TTS

Splits cleaned text into ~4000-char chunks at sentence boundaries. Sends each chunk to OpenAI or ElevenLabs TTS API. Concatenates resulting MP3 chunks (MP3 frames are self-delimiting). Uploads to Tigris with content-addressed filename.

### Error Handling

Failed jobs retry with exponential backoff (2min, 4min, 8min). After 3 attempts, the episode enters error state. Users can manually retry from the UI.

## Frontend

Three pages:
1. **Feed list** (/) — Admin view. Enter admin token, see all feeds, create/delete.
2. **Feed view** (/feeds/[token]) — Submit URLs, see episode list with status badges, polls every 5s.
3. **Episode detail** (/feeds/[token]/episodes/[id]) — Full info, audio player, retry button.

## Configuration

All configuration via environment variables. See `.env.example` for the full list.

Required: `DATABASE_URL`, `ANTHROPIC_API_KEY`, `ADMIN_TOKEN`, S3/Tigris credentials, and at least one TTS provider key.
