# Deployment

The app runs as a single Fly.io machine: one Rust binary serving HTTP, a SQLite database on a mounted volume, and Litestream replicating that database to a Tigris (S3-compatible) bucket. The same bucket also holds generated audio and cover images. No Vercel, no separate frontend host.

This document covers both the one-time provisioning and the steady-state operations I need when redeploying or rotating credentials.

## Accounts and API Keys

You need four external providers. Keep all keys in `.env` locally; `scripts/sync-secrets.sh push` copies them to Fly secrets.

### 1. Fly.io — hosting, volume, Tigris bucket

- Sign up at https://fly.io and install `flyctl`: `brew install flyctl` or `curl -L https://fly.io/install.sh | sh`.
- Create a deploy token: `fly tokens create deploy -x 999999h` (or use the dashboard: **Tokens → Create access token**). Put it in `.env` as `FLY_API_TOKEN`; the `fly` CLI picks it up automatically, so you don't need an interactive `fly auth login` inside this repo.
- Tigris (S3-compatible storage) is provisioned via `fly storage create` below; the credentials are auto-managed as Fly secrets.

### 2. Anthropic — Claude (article/PDF cleanup, PDF extraction, optional summarize)

- Go to https://console.anthropic.com/settings/keys.
- **Create Key**, name it (e.g. `tts-podcast`), copy the `sk-ant-…` value.
- Fund the workspace with some credit — the app uses Sonnet for articles and Opus for academic papers, which dominates per-episode cost.
- Rotate: create a new key, update `ANTHROPIC_API_KEY` in `.env`, run `./scripts/sync-secrets.sh push`, then revoke the old key in the console.

### 3. Google Cloud — Text-to-Speech

- Create or pick a project at https://console.cloud.google.com.
- Enable the **Cloud Text-to-Speech API**: https://console.cloud.google.com/apis/library/texttospeech.googleapis.com → **Enable**.
- Create an API key: **APIs & Services → Credentials → Create credentials → API key**. Copy the `AIza…` value.
- Restrict the key (recommended): **Edit → API restrictions → Restrict key → Cloud Text-to-Speech API**. Don't add an HTTP referrer restriction — the calls come from Fly, not a browser.
- This is `GOOGLE_TTS_API_KEY`. Billing must be enabled on the project; Journey voices are paid.

### 4. Google AI Studio — Gemini (cover art, optional clean/summarize provider)

- Go to https://aistudio.google.com/apikey.
- **Create API key** (can be in the same GCP project as TTS or a separate one). Copy the `AIza…` value.
- This is `GOOGLE_STUDIO_API_KEY`. It's a distinct key from the TTS one — AI Studio and Cloud TTS use different API surfaces.
- Image generation uses `gemini-2.5-flash-image`; the free tier is usually sufficient for personal use.

### Admin token

Generate one locally:

```bash
openssl rand -hex 32
```

Put the result in `.env` as `ADMIN_TOKEN`. This gates feed creation/deletion.

## Environment File

`.env.example` is the template. Copy it and fill in the real values:

```bash
cp .env.example .env
# edit .env
set -a; source .env; set +a
```

Variables fall into four buckets:

- **Fly CLI**: `FLY_API_TOKEN`.
- **Tigris credentials** (filled in automatically after `fly storage create`): `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT_URL_S3`, `AWS_REGION`, `BUCKET_NAME`.
- **Application secrets**: `ANTHROPIC_API_KEY`, `GOOGLE_TTS_API_KEY`, `GOOGLE_STUDIO_API_KEY`, `ADMIN_TOKEN`, `PUBLIC_URL`.
- **Local-only / tunables**: `DATABASE_URL`, `HOST`, `PORT`, `GOOGLE_TTS_VOICE`, `WORKER_POLL_INTERVAL_SECS`, `MAX_JOB_ATTEMPTS`, `GENERATE_IMAGES`, `STATIC_DIR`.

### scripts/sync-secrets.sh

```bash
./scripts/sync-secrets.sh status   # show which keys are set locally vs on Fly
./scripts/sync-secrets.sh push     # push app secrets from .env to Fly
```

`push` only syncs the application secrets (`ANTHROPIC_API_KEY`, `GOOGLE_TTS_API_KEY`, `GOOGLE_STUDIO_API_KEY`, `ADMIN_TOKEN`, `PUBLIC_URL`). Tigris credentials are managed by `fly storage` and intentionally excluded. `DATABASE_URL` comes from `fly.toml`.

## First-Time Provisioning

### 1. Create the Fly app

```bash
fly launch --no-deploy
```

Pick an app name and region (I use `sjc`). This writes/updates `fly.toml`. The existing `fly.toml` is set up for the `tchajed-podcast` app in `sjc` with a 512MB shared-CPU VM, auto-start/stop, and a `/data` volume mount.

### 2. Create the SQLite volume

```bash
fly volumes create podcast_data --region sjc --size 1
```

1 GB is plenty — the DB holds only metadata and text; audio goes to Tigris.

### 3. Create the Tigris bucket

```bash
fly storage create --public --name my-tts-podcast-audio
```

`--public` is required because podcast clients fetch MP3s directly from the bucket URL. This sets `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT_URL_S3`, `AWS_REGION`, and `BUCKET_NAME` as Fly secrets and prints the same values so you can copy them into local `.env`.

### 4. Push application secrets

```bash
./scripts/sync-secrets.sh push
```

### 5. Deploy

```bash
fly deploy --depot=false
```

`--depot=false` forces Fly's own builders instead of Depot's gRPC/SNI-based remote builder, which hangs in this environment. The first build compiles all Rust deps from scratch (~20 min); subsequent deploys reuse the cargo-chef dependency layer (~2 min).

Alternative: `fly deploy --local-only` builds locally if you have Docker.

The Dockerfile is multi-stage: Bun builds the SvelteKit static bundle, cargo-chef + Rust builds `tts-podcast-backend`, and the runtime image layers on `poppler-utils`, Litestream 0.5.11, and the static files. `start.sh` runs `litestream restore -if-replica-exists` on first boot, then `litestream replicate -exec /usr/local/bin/backend` so Litestream supervises the app process.

### 6. Sanity checks

```bash
curl https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN"
# → []
```

Visit `https://my-tts-podcast.fly.dev` — the SvelteKit UI should load.

### 7. Create a feed

```bash
curl -X POST https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"slug":"reading-list","title":"Reading List","description":"Articles and papers"}'
```

The response has `feed_token` and `rss_url`. Paste the RSS URL into a podcast client.

### 8. Submit a test episode

```bash
FEED_TOKEN=<from above>

curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://www.anthropic.com/engineering/managed-agents"}'

# With summarization
curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes" \
  -H "Content-Type: application/json" \
  -d '{"url":"https://arxiv.org/abs/2401.03468","summarize":true}'

# PDF upload (preserve source link)
curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes/pdf" \
  -F "file=@paper.pdf" \
  -F "title=My Paper" \
  -F "source_url=https://example.com/paper.pdf"
```

## Custom Domain (optional, via Cloudflare)

1. Point the domain's nameservers to Cloudflare.
2. Add a `CNAME podcast.yourdomain.com → my-tts-podcast.fly.dev` (proxied, orange cloud).
3. Update `PUBLIC_URL` in `.env` and push: `./scripts/sync-secrets.sh push`.

Cloudflare gives free TLS, DDoS protection, and edge caching of the RSS feed. Fly's shared IPv4 + IPv6 are sufficient — no dedicated IP needed.

## Monitoring and Ops

```bash
fly logs              # stream backend logs
fly status            # machine health
fly ssh console       # shell into the VM
fly volumes list      # volume status
fly secrets list      # what's currently set
```

Inside the VM the SQLite DB is at `/data/podcast.db` and Litestream logs to stdout alongside the app.

## Disaster Recovery

Litestream streams the WAL to `s3://$BUCKET_NAME/litestream/podcast.db` continuously. To recover:

1. Delete the machine (or recreate the volume).
2. On first boot, `start.sh` runs `litestream restore -if-replica-exists /data/podcast.db`, which pulls the latest snapshot + WAL from Tigris.
3. Audio and images are independently stored in the same bucket and aren't affected.

Auto-stop can drop a few seconds of un-replicated WAL when a machine pauses. For a personal feed this is fine.

## Updating

Normal deploys:

```bash
git push            # if you use GitHub Actions, adjust as needed
fly deploy --depot=false
```

Rotating an API key:

1. Create a new key at the provider.
2. Update `.env`.
3. `./scripts/sync-secrets.sh push` (Fly restarts the machine automatically when secrets change).
4. Revoke the old key.

Rotating `ADMIN_TOKEN` does the same thing; any existing `feed_token`s continue to work (they're stored in the DB).

## Cost

Rough monthly steady state for personal use:

| Component | Cost |
|---|---|
| Fly machine (512MB, auto-stop) | ~$3–5 |
| Fly volume (1 GB) | ~$0.15 |
| Tigris storage + egress | ~$1–2 |
| **Infrastructure** | **~$5–8** |

Per-episode variable costs:

- Claude cleanup: $0.01–$1.50 (Sonnet cheap, Opus on long papers expensive).
- Claude PDF extraction: ~$0.20–0.60 per 20-page paper.
- Google TTS: ~$0.04 per 20k characters (Journey voices).
- Gemini cover art: ~$0.04 per episode.
