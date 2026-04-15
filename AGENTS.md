# AGENTS.md

## Languages & tools

- **Backend**: Rust (Cargo workspace: `tts-lib`, `tts-cli`, `server`). Axum HTTP, SQLite (WAL) with Litestream replication to Tigris (S3).
- **Frontend**: SvelteKit (Svelte 5) + TypeScript, built with Vite, package manager is Bun (`bun.lock`). `adapter-static` output served by the Rust binary.
- **External APIs**: Anthropic Claude, Google Cloud TTS, Google AI Studio (Gemini).
- **Deployment**: Fly.io, single app, Docker image with `poppler-utils`.

## Episode pipeline stages

Job types (from `jobs.job_type`): `scrape`, `pdf`, `clean`, `summarize`, `tts`, `image`.

Episode statuses (from `episodes.status`): `pending`, `scraping`, `cleaning`, `summarizing`, `tts`, `done`, `error`.

The `scraping` status covers both `scrape` and `pdf` job types. `summarize` is optional. `image` runs after `done` and is non-fatal.
