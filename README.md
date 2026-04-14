# TTS Podcast

> **TODO (2026-04-14):** Clean up the Tigris public-hostname handling.
> During debugging we tried three hostnames (`fly.storage.tigris.dev`,
> `t3.storage.dev`, `t3.tigrisfiles.io`) and churned on ACLs, bucket
> policies, and the `--public` flag before landing on `t3.tigrisfiles.io`.
> Confidence in the current code is low — see
> [TIGRIS_INVESTIGATION.md](TIGRIS_INVESTIGATION.md) for the full log.
> Open items: verify playback end-to-end, hoist the public hostname into
> config, decide whether the `acl(public-read)` on uploads is actually
> needed, and consider switching to presigned URLs.

A self-hosted web app that turns web articles, arXiv papers, and PDFs into a private podcast feed. Submit a URL or upload a PDF; the backend scrapes or extracts the text, cleans it with an LLM, optionally summarizes it, synthesizes speech with Google Cloud TTS, generates cover art with Gemini, and publishes an RSS feed that any podcast client can subscribe to.

There is no user login. Access is controlled by a per-feed secret token embedded in the RSS URL and API calls, plus an admin token for feed management.

## Repository Layout

```
backend/          Rust workspace
  tts-lib/          Shared library: providers (Claude/Gemini), TTS, scrape, PDF, cleanup, summarize
  tts-cli/          Command-line tool for running the pipeline locally
  server/           Axum HTTP server + background worker (the binary that ships)
frontend/         SvelteKit (Svelte 5) SPA, built static and served by the backend
scripts/          Helper scripts (sync-secrets.sh)
Dockerfile        Multi-stage build (Bun frontend, cargo-chef backend, Litestream runtime)
fly.toml          Fly.io app config
litestream.yml    Continuous SQLite backup to Tigris (S3)
start.sh          Runtime entrypoint: restore DB, run Litestream supervising the backend
```

## Architecture

- **Backend**: Rust / Axum. Single binary serving the API, the RSS feed, the SvelteKit static build, and running the job worker inline.
- **Database**: SQLite on a Fly volume, WAL mode, embedded migrations.
- **Backups**: Litestream replicates the SQLite WAL to Tigris continuously.
- **Storage**: Tigris (S3-compatible) for audio MP3s, cover images, and DB backups.
- **Hosting**: A single Fly.io app — no Vercel, no separate worker.

## Local Development

### Prerequisites

- Rust 1.94+ (matches the Dockerfile)
- Bun
- `poppler-utils` (for PDF handling)
- API keys from Anthropic and Google (see [DEPLOYMENT.md](DEPLOYMENT.md) for how to obtain them)

### Setup

```bash
cp .env.example .env
# Fill in API keys and generate an admin token: openssl rand -hex 32
set -a; source .env; set +a
```

### Backend

```bash
cd backend
cargo run -p tts-podcast-backend
```

The server listens on `http://localhost:8080`, creates the SQLite database if missing, and runs migrations on startup.

### Frontend

```bash
cd frontend
bun install
bun run dev      # dev server on :5173 — set VITE_API_BASE_URL=http://localhost:8080
# or
bun run build    # produces frontend/build, which the backend serves via STATIC_DIR
```

### CLI

The `tts-cli` crate runs the pipeline end-to-end without the server — handy for debugging cleanup prompts or trying a new provider against a single URL:

```bash
cd backend
cargo run -p tts-cli -- --help
```

## Pipeline Overview

```
URL  → scrape → clean → [summarize] → tts → done → image
PDF  → pdf    → clean → [summarize] → tts → done → image
```

- **scrape**: readability for articles; arXiv API + ar5iv.org HTML for papers.
- **pdf**: Claude vision reads the PDF as a document (handles two-column layouts).
- **clean**: Claude (or Gemini, configurable) removes navigation junk, fixes encoding, rewrites equations as spoken English for academic sources.
- **summarize** (optional, per-episode flag): LLM produces a shorter spoken version stored as `transcript`. If present, TTS uses this instead of the cleaned text.
- **tts**: Google Cloud TTS, chunked at sentence boundaries, MP3 frames concatenated.
- **image**: Gemini 2.5 Flash Image generates cover art from a short summary. Runs after the episode is already `done` — failures are non-fatal.

Each stage is a job row with exponential-backoff retry.

## API

See [DESIGN.md](DESIGN.md) for the full surface. The useful endpoints:

- `POST /api/v1/feeds` — create a feed (admin)
- `GET /api/v1/feeds/:token` — feed + episodes (token-auth)
- `POST /api/v1/feeds/:token/episodes` — submit a URL. Body: `{ url, summarize?, tts_provider? }`
- `POST /api/v1/feeds/:token/episodes/pdf` — upload a PDF (multipart; accepts `source_url` to preserve the original link)
- `GET /feed/:token/rss.xml` — the RSS feed to hand to a podcast client

## Deployment

Everything runs on one Fly.io app with a mounted volume and a Tigris bucket. See [DEPLOYMENT.md](DEPLOYMENT.md) for the full setup, including how to create and rotate each API key.

## License

Private / personal use.
