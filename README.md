# TTS Podcast

A self-hosted web app that converts web articles and arXiv papers into podcast episodes. Submit URLs via a web UI; the backend scrapes, cleans text with Claude, synthesizes audio via TTS, and publishes to a private RSS feed for any podcast client.

## Architecture

- **Backend**: Rust (Axum) — API server + background worker
- **Frontend**: SvelteKit + TypeScript (Bun)
- **Database**: Postgres
- **Storage**: Tigris (S3-compatible) for audio files
- **Hosting**: Fly.io (backend), Vercel (frontend)

## Quick Start (Local Development)

### Prerequisites

- Rust 1.75+
- Bun
- Postgres (local or Docker)
- API keys: Anthropic, OpenAI and/or ElevenLabs

### Backend

```bash
cp .env.example .env
# Edit .env with your credentials

cd backend
cargo run
```

The server starts on `http://localhost:8080`. It runs migrations automatically on startup.

### Frontend

```bash
cd frontend
bun install
VITE_API_BASE_URL=http://localhost:8080 bun run dev
```

Opens on `http://localhost:5173`.

### Local Postgres (Docker)

```bash
docker run -d --name tts-pg \
  -e POSTGRES_USER=tts -e POSTGRES_PASSWORD=tts -e POSTGRES_DB=tts_podcast \
  -p 5432:5432 postgres:16
```

Set `DATABASE_URL=postgres://tts:tts@localhost:5432/tts_podcast` in `.env`.

## How It Works

1. **Submit a URL** via the web UI (article or arXiv paper)
2. **Scrape**: Fetches the page; uses ar5iv.org for arXiv papers (HTML, not PDF)
3. **Clean**: Claude removes navigation debris, converts equations to spoken English
4. **TTS**: OpenAI or ElevenLabs converts cleaned text to MP3 audio
5. **Publish**: Audio uploaded to Tigris; episode appears in the RSS feed

Each stage runs as a background job with retry and exponential backoff.

## API

See [DESIGN.md](DESIGN.md) for full API documentation.

Key endpoints:
- `POST /api/v1/feeds` — Create a feed (admin)
- `GET /api/v1/feeds/:token` — Get feed + episodes (token is auth)
- `POST /api/v1/feeds/:token/episodes` — Submit a URL for processing
- `GET /feed/:token/rss.xml` — RSS feed for podcast clients

## Deployment

See the [Deployment Plan](DEPLOYMENT.md) for step-by-step instructions.

## Test URLs

These are good URLs for testing the pipeline:

- **Article**: https://www.anthropic.com/engineering/managed-agents
- **arXiv (Nola)**: https://arxiv.org/abs/2401.03468
- **arXiv (Spanner)**: look up the Spanner paper on Google Scholar

## License

Private / personal use.
