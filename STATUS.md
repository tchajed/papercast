# Implementation Status Report

## What's Done

### Backend (Rust/Axum) — Complete, compiles cleanly
- **Database**: SQLite with WAL mode, auto-creation, embedded migrations
- **API routes**: Feed CRUD, episode submission (URL + PDF upload), status, retry, RSS
- **Processing pipeline**:
  - **Scrape**: Article readability + arXiv (metadata API + ar5iv HTML)
  - **PDF**: Claude vision extraction (handles two-column layouts)
  - **Clean**: Claude with source-specific prompts (Sonnet for articles, Opus for academic)
  - **TTS**: OpenAI, ElevenLabs, and Google Cloud TTS with text chunking
  - **Image**: Gemini cover art generation (non-fatal, runs after `done`)
  - **Storage**: Tigris S3 upload (audio + images, content-addressed keys)
- **Worker**: Inline job execution, exponential backoff, stage transitions in transactions
- **Static serving**: Axum serves SvelteKit build output via `tower-http` ServeDir
- **Exact MP3 duration** via `mp3-duration` crate

### Frontend (SvelteKit + Bun) — Complete, type-checks cleanly
- Three pages: feed list (admin), feed view (URL submit + PDF upload), episode detail
- Google TTS as third provider option
- Cover image thumbnails in episode list, larger in detail view
- Status polling every 5 seconds for in-progress episodes
- Built as static SPA (adapter-static with fallback)

### Deployment — Complete
- Multi-stage Dockerfile (Bun frontend build + Rust backend build)
- Litestream backup of SQLite to Tigris
- start.sh with auto-restore on first boot
- fly.toml with volume mount for SQLite persistence
- Single Fly.io app — no Vercel or separate frontend hosting

### Documentation — Complete
- README.md, DESIGN.md, DEPLOYMENT.md, this STATUS.md

## What Hasn't Been Tested

The code compiles and type-checks but hasn't been run end-to-end because this environment doesn't have:
- API keys (Anthropic, OpenAI, Google)
- A Tigris bucket
- A Fly.io account

## Known Limitations

1. **PDF extraction**: Uses Claude vision on the raw PDF (sends PDF as document). For very large PDFs (50+ pages), this may hit API limits. Consider page-by-page rendering with `pdfium-render` for better control.

2. **Sentence splitting**: Simple split on `.!?` — can be confused by abbreviations, decimal numbers, or URLs.

3. **CORS**: Currently allows all origins. For production, restrict to the app's domain.

4. **Litestream + auto-stop**: Fly's auto-stop pauses the machine. When it restarts, Litestream resumes from the last WAL position. There's a small window where WAL entries written just before stop might not be replicated. For a personal app this is acceptable.

5. **No feed-level artwork**: RSS `<itunes:image>` at the channel level is not set — podcast clients show blank covers until per-episode images are generated.

## Test URLs

1. **Article**: `https://www.anthropic.com/engineering/managed-agents`
2. **arXiv (Nola)**: `https://arxiv.org/abs/2401.03468` — "Later-Free Ghost State for Verifying Termination in Iris"
3. **arXiv (Spanner)**: Look up on Google Scholar
4. **PDF**: Download any two-column academic paper and use the PDF upload form

## Next Steps

1. **Deploy to Fly.io** — Follow DEPLOYMENT.md
2. **Test end-to-end** with the test URLs above
3. **Tune Claude prompts** based on real output quality
4. **Add custom domain** via Cloudflare (optional)
5. **Consider adding**:
   - Feed-level artwork
   - More robust sentence splitting (NLP tokenizer)
   - Rate limiting
   - Email ingest (Cloudflare Email Routing)
   - PWA share target for mobile
