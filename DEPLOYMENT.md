# Deployment Plan

Everything runs in a single Fly.io app. No Vercel, no separate frontend hosting.

## Accounts Required

1. **Fly.io** — Hosting, SQLite volume, Tigris storage
   - Sign up at https://fly.io
   - Install `flyctl`: `brew install flyctl` or `curl -L https://fly.io/install.sh | sh`
   - Run `fly auth login`

2. **Anthropic** — Claude API for text cleanup and PDF extraction
   - API key from https://console.anthropic.com

3. **OpenAI** (recommended) — TTS
   - API key from https://platform.openai.com/api-keys

4. **Google Cloud** (optional) — Google TTS + Gemini image generation
   - API key from https://aistudio.google.com/apikey
   - One key covers both TTS and Gemini

5. **ElevenLabs** (optional) — Alternative TTS
   - API key from https://elevenlabs.io
   - Note your preferred voice ID

6. **Domain registrar** (optional) — For custom domain
   - Any registrar works; Cloudflare recommended for free proxying

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
  OPENAI_API_KEY="sk-..." \
  ADMIN_TOKEN="$ADMIN_TOKEN" \
  PUBLIC_URL="https://my-tts-podcast.fly.dev"

# Optional: Google (TTS + image generation)
fly secrets set GOOGLE_API_KEY="..."

# Optional: ElevenLabs
fly secrets set \
  ELEVENLABS_API_KEY="..." \
  ELEVENLABS_VOICE_ID="..."
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
- TTS: ~$0.30 per 20K-char article (OpenAI), varies by provider
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
