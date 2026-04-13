# TTS Podcast

A self-hosted web app that converts web articles, arXiv papers, and PDFs into podcast episodes. Submit URLs or upload PDFs via a web UI; the backend scrapes or extracts text, cleans it with Claude, synthesizes audio via TTS, generates cover art, and publishes to a private RSS feed for any podcast client.

## Architecture

- **Backend**: Rust (Axum) — API server + background worker + static file serving
- **Frontend**: SvelteKit (static SPA, served by Axum)
- **Database**: SQLite with WAL mode, backed up via Litestream to Tigris
- **Storage**: Tigris (S3-compatible) for audio files, cover images, and DB backups
- **Hosting**: Single Fly.io app (everything in one)

## Quick Start (Local Development)

### Prerequisites

- Rust 1.75+
- Bun
- API keys: Anthropic, Google (for TTS and Gemini image generation)

### Setup

```bash
cp .env.example .env
# Edit .env with your API keys and credentials
# Generate an admin token: openssl rand -hex 32
```

The `.env` file is gitignored. It also holds `FLY_API_TOKEN` for Fly.io CLI access. Source it before running anything:

```bash
set -a; source .env; set +a
```

### Backend

```bash
cd backend
cargo run
```

The server starts on `http://localhost:8080`. It creates the SQLite database and runs migrations automatically.

### Frontend

```bash
cd frontend
bun install
bun run build    # Build static files (served by backend)
bun run dev      # Or run dev server on :5173 (set VITE_API_BASE_URL=http://localhost:8080)
```

## How It Works

1. **Submit a URL** (article or arXiv paper) or **upload a PDF** via the web UI
2. **Scrape/Extract**: Articles via readability; arXiv via ar5iv.org HTML; PDFs via Claude vision
3. **Clean**: Claude removes navigation debris, converts equations to spoken English
4. **TTS**: Google Cloud TTS converts cleaned text to MP3
5. **Publish**: Audio uploaded to Tigris; episode appears in the RSS feed
6. **Cover Art** (optional): Gemini generates a per-episode illustration

Each stage runs as a background job with retry and exponential backoff.

## API

See [DESIGN.md](DESIGN.md) for full API documentation.

Key endpoints:
- `POST /api/v1/feeds` — Create a feed (admin)
- `GET /api/v1/feeds/:token` — Get feed + episodes (token is auth)
- `POST /api/v1/feeds/:token/episodes` — Submit a URL
- `POST /api/v1/feeds/:token/episodes/pdf` — Upload a PDF
- `GET /feed/:token/rss.xml` — RSS feed for podcast clients

## Deployment

See [DEPLOYMENT.md](DEPLOYMENT.md) for step-by-step instructions. Single Fly.io app — no Vercel or separate frontend hosting needed.

## Test URLs

- **Article**: https://www.anthropic.com/engineering/managed-agents
- **arXiv (Nola)**: https://arxiv.org/abs/2401.03468
- **arXiv (Spanner)**: look up on Google Scholar

## License

Private / personal use.
