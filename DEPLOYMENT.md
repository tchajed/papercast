# Deployment Plan

Everything runs in a single Fly.io app. No Vercel, no separate frontend hosting.

## Accounts Required

1. **Fly.io** — Hosting, SQLite volume, Tigris storage
   - Sign up at https://fly.io
   - Install `flyctl`: `brew install flyctl` or `curl -L https://fly.io/install.sh | sh`
   - Run `fly auth login`

2. **Anthropic** — Claude API for text cleanup and PDF extraction
   - API key from https://console.anthropic.com

3. **Google Cloud** — Google TTS + Gemini image generation
   - API key from https://aistudio.google.com/apikey
   - One key covers both TTS and Gemini

4. **Domain registrar** (optional) — For custom domain
   - Any registrar works; Cloudflare recommended for free proxying

## Environment Setup

All secrets and config live in `.env` (gitignored). Copy the template and fill in your values:

```bash
cp .env.example .env
# Edit .env with your API keys
# Generate an admin token: openssl rand -hex 32
```

Source it before running any commands (backend, `fly`, or the sync script):

```bash
set -a; source .env; set +a
```

The `.env` file includes `FLY_API_TOKEN`, which the `fly` CLI picks up automatically — no need for `fly auth login` in environments where you've set it.

To push API key secrets to Fly.io after setting them in `.env`:

```bash
./scripts/sync-secrets.sh push    # pushes ANTHROPIC_API_KEY, GOOGLE_API_KEY, ADMIN_TOKEN, PUBLIC_URL
./scripts/sync-secrets.sh status  # shows what's set locally vs on Fly
```

## Step-by-Step Deployment

### 1. Create the Fly.io App

```bash
cd tts-podcast
fly launch --no-deploy
# Choose a unique app name, e.g. "my-tts-podcast"
# Select region (sjc recommended for US West)
```

### 2. Create SQLite Volume

```bash
fly volumes create podcast_data --region sjc --size 1
```

### 3. Create Tigris Storage Bucket

```bash
fly storage create --public --name my-tts-podcast-audio
```

This automatically sets `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_ENDPOINT_URL_S3`, `BUCKET_NAME`, and `AWS_REGION` as Fly secrets.

### 4. Set Secrets

```bash
# Generate a random admin token
ADMIN_TOKEN=$(openssl rand -hex 32)
echo "Save this admin token: $ADMIN_TOKEN"

fly secrets set \
  ANTHROPIC_API_KEY="sk-ant-..." \
  GOOGLE_API_KEY="..." \
  ADMIN_TOKEN="$ADMIN_TOKEN" \
  PUBLIC_URL="https://my-tts-podcast.fly.dev"
```

Note: `DATABASE_URL` is set in `fly.toml` env section, not as a secret.

### 5. Deploy

```bash
fly deploy
```

This builds a multi-stage Docker image (Bun builds frontend, Rust builds backend), adds Litestream, and deploys. Migrations run automatically on startup. Litestream begins backing up SQLite to Tigris immediately.

Verify:
```bash
curl https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN"
# Should return []
```

Visit `https://my-tts-podcast.fly.dev` in a browser — the SvelteKit UI should load.

### 6. Create Your First Feed

```bash
curl -X POST https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"slug": "reading-list", "title": "Reading List", "description": "Articles and papers"}'
```

The response includes a `feed_token` and `rss_url`. Add the RSS URL to your podcast client.

### 7. Submit a Test Episode

```bash
FEED_TOKEN="<feed_token from above>"

# URL submission
curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://www.anthropic.com/engineering/managed-agents"}'

# PDF upload
curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes/pdf" \
  -F "file=@paper.pdf" \
  -F "title=My Paper"
```

## Custom Domain (Optional, Cloudflare)

1. Register a domain (or use existing one)
2. Point nameservers to Cloudflare (free plan)
3. Add DNS record: `CNAME podcast.yourdomain.com → my-tts-podcast.fly.dev` (proxied)
4. Update `PUBLIC_URL`: `fly secrets set PUBLIC_URL=https://podcast.yourdomain.com`

Cloudflare proxying gives free TLS, DDoS protection, and edge caching of RSS feeds. No dedicated IPv4 needed — Fly's shared IPv4 + IPv6 work fine.

## Cost Estimates

| Component | Cost |
|---|---|
| Fly machine (512MB, auto-stop) | ~$3–5/mo |
| Fly volume (1GB SQLite) | $0.15/mo |
| Tigris (audio + images + Litestream) | ~$1–2/mo |
| Shared IPv4 + IPv6 | Free |
| **Infrastructure total** | **~$5–8/mo** |

Per-episode costs:
- Claude cleanup: $0.01–$1.50 (Sonnet for articles, Opus for papers)
- PDF extraction: ~$0.20–0.60 per 20-page paper
- TTS: ~$0.04 per 20K-char article (Google Cloud TTS)
- Image generation: ~$0.04 per episode (Gemini)

## Monitoring

```bash
fly logs              # Stream backend logs
fly status            # Check app health
fly ssh console       # SSH into the VM
fly volumes list      # Check volume status
```

## Disaster Recovery

Litestream continuously streams the SQLite WAL to Tigris. To restore:

1. Delete the volume (or create a new machine)
2. `start.sh` automatically restores from the latest Litestream backup on first boot
3. Audio files are independently stored in Tigris and unaffected
