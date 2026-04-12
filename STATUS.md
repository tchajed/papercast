# Implementation Status Report

## What's Done

### Backend (Rust/Axum) — Complete
- **Database schema**: Two migrations covering feeds, episodes, and jobs tables with proper constraints and indexes
- **Configuration**: `AppConfig` loaded from environment variables with sensible defaults
- **API routes**:
  - Feed CRUD (create, list, get, delete) with admin token auth
  - Episode submission, status checking, deletion, and retry
  - RSS feed generation with iTunes podcast extensions
- **Processing pipeline**:
  - **Scrape**: Article readability extraction and arXiv support (metadata from arXiv API, HTML from ar5iv.org)
  - **Clean**: Claude API integration with source-type-specific prompts (article vs. academic paper)
  - **TTS**: Both OpenAI (tts-1-hd) and ElevenLabs (Flash v2.5) with text chunking at sentence boundaries
  - **Storage**: Tigris S3 upload with content-addressed filenames
- **Worker**: Postgres-backed job queue with `FOR UPDATE SKIP LOCKED`, exponential backoff retry, stage transitions in transactions
- **Compiles**: `cargo check` passes cleanly

### Frontend (SvelteKit + Bun) — Complete
- Three pages: feed list (admin), feed view (submit URLs + episode list), episode detail
- Status polling every 5 seconds for in-progress episodes
- Audio player for completed episodes
- Retry button for failed episodes
- Type-checks cleanly with `svelte-check`

### Deployment Config — Complete
- Dockerfile (multi-stage Rust build)
- fly.toml for Fly.io
- .env.example with all configuration documented

### Documentation — Complete
- README.md — Quick start and overview
- DESIGN.md — Human-readable architecture and API docs
- DEPLOYMENT.md — Step-by-step deployment with all accounts and commands

## What Hasn't Been Tested

The code compiles and type-checks but hasn't been run end-to-end because this environment doesn't have:
- A running Postgres instance
- Anthropic/OpenAI/ElevenLabs API keys configured
- Tigris storage bucket

## Known Limitations / Things to Address Before First Deploy

1. **readability crate**: The `readability` crate (v0.3) may not handle all HTML well. If extraction quality is poor for specific sites, consider switching to a different crate or using a headless browser.

2. **ar5iv availability**: ar5iv.org occasionally has downtime or slow responses for less popular papers. The retry mechanism handles transient failures.

3. **Claude model choice**: Currently both article and arXiv cleanup use `claude-sonnet-4-6`. For dense academic papers, switching arXiv to Opus would improve equation-to-speech conversion quality at higher cost.

4. **Text chunking**: The sentence-splitting logic is simple (splits on `.!?`). It could be confused by abbreviations (e.g., "Dr. Smith"), decimal numbers, or URLs. A more robust NLP sentence tokenizer would help.

5. **MP3 duration**: Currently estimated from word count (150 wpm). Exact duration requires parsing MP3 frame headers.

6. **CORS**: Currently allows all origins. For production, restrict to the frontend domain.

## Suggested Test URLs

Once deployed, test with these URLs in order of complexity:

1. **Simple article**: `https://www.anthropic.com/engineering/managed-agents`
   - Tests basic article scraping and cleanup

2. **arXiv paper (Nola)**: `https://arxiv.org/abs/2401.03468`
   - "Nola: Later-Free Ghost State for Verifying Termination in Iris"
   - Tests arXiv metadata fetch, ar5iv HTML extraction, academic cleanup

3. **arXiv paper (Spanner)**: Look up "Spanner: Google's Globally-Distributed Database" on Google Scholar or arXiv
   - Classic systems paper, good test for longer content

## Next Steps

1. **Deploy to Fly.io** — Follow DEPLOYMENT.md
2. **Test the pipeline end-to-end** with the test URLs above
3. **Tune prompts** — The Claude cleanup prompts may need iteration based on real output quality
4. **Add frontend environment config** — Set up Vercel with `VITE_API_BASE_URL`
5. **Add to podcast client** — Test RSS feed in Overcast, Apple Podcasts, or Pocket Casts
6. **Consider adding**:
   - Feed artwork (iTunes image tag)
   - More robust sentence splitting
   - Rate limiting on episode submission
   - Exact MP3 duration calculation
