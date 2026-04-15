# TODOs

## Needs user input

- Name the app, add a logo.
- Add a little audio boop between sections, the way AI Daily Brief does (but
  without copying their sound effect). Not sure where to get such a sound
  effect.

## Planned

- Add time remaining estimation. As each stage completes, we get more info
  for the later stages to make the estimate better (e.g., we need the
  content length to have any real estimate).
- Fix the chapter timings. For Spanner these are definitely incorrect.
  `parse_sections` now tolerates leading whitespace on `## ` headers and
  logs the detected section count, but the root cause still needs a
  concrete repro — check whether cleaned_text for Spanner has `## ` at all
  and whether per-chunk durations are roughly uniform.

## Done

- Read-only admin interface: `GET /api/v1/admin/{status,jobs,usage}` gated
  by `ADMIN_TOKEN`. TTS chunk progress is exposed per-episode in the feed
  view and per-job in `/admin/jobs`.
- AI cost tracking: `ai_usage` table records every AI call with tokens/chars.
  `tts-cli costs` queries the deployed server and prints a USD breakdown.
- API documentation for LLM consumption: [llms.txt](llms.txt) documents
  every endpoint + the SQLite schema.
- Time zone issue: investigated, only affects old episodes. All new
  timestamps are stored UTC and the frontend appends `Z` before formatting.
- Delete-episode button on the frontend episode page.
