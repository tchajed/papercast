# Deployment Plan

## Accounts Required

You need accounts with the following services:

1. **Fly.io** — Backend hosting + Postgres + Tigris storage
   - Sign up at https://fly.io
   - Install `flyctl`: `brew install flyctl` or `curl -L https://fly.io/install.sh | sh`
   - Run `fly auth login`

2. **Anthropic** — Claude API for text cleanup
   - API key from https://console.anthropic.com

3. **OpenAI** (recommended) — TTS
   - API key from https://platform.openai.com/api-keys

4. **ElevenLabs** (optional) — Alternative TTS
   - API key from https://elevenlabs.io
   - Note your preferred voice ID

5. **Vercel** (for frontend) — Or any static hosting
   - Sign up at https://vercel.com
   - Install: `npm i -g vercel`

## Step-by-Step Deployment

### 1. Create the Fly.io App

```bash
cd tts-podcast
fly launch --no-deploy
# Choose a unique app name, e.g. "my-tts-podcast"
# Select region (sjc recommended for US West)
# Say no to database for now (we'll create separately)
```

Edit `fly.toml` to update the app name if needed.

### 2. Create Postgres Database

```bash
fly postgres create --name my-tts-podcast-db --region sjc --initial-cluster-size 1 --vm-size shared-cpu-1x --volume-size 1
fly postgres attach my-tts-podcast-db
```

This automatically sets `DATABASE_URL` as a Fly secret.

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

# Optional: ElevenLabs
fly secrets set \
  ELEVENLABS_API_KEY="..." \
  ELEVENLABS_VOICE_ID="..."
```

### 5. Deploy Backend

```bash
fly deploy
```

This builds the Docker image and deploys it. Migrations run automatically on startup.

Verify it's running:
```bash
curl https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN"
# Should return []
```

### 6. Deploy Frontend

```bash
cd frontend

# Set the API URL
echo "VITE_API_BASE_URL=https://my-tts-podcast.fly.dev" > .env.production

vercel --prod
```

Or deploy to any static hosting. The frontend is a pure client-side app that talks to the backend API.

### 7. Create Your First Feed

```bash
curl -X POST https://my-tts-podcast.fly.dev/api/v1/feeds \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"slug": "reading-list", "title": "Reading List", "description": "Articles and papers"}'
```

The response includes a `feed_token` and `rss_url`. Add the RSS URL to your podcast client (Overcast, Apple Podcasts, etc.).

### 8. Submit a Test Episode

```bash
FEED_TOKEN="<feed_token from above>"

curl -X POST "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN/episodes" \
  -H "Content-Type: application/json" \
  -d '{"url": "https://www.anthropic.com/engineering/managed-agents"}'
```

Poll for status:
```bash
curl "https://my-tts-podcast.fly.dev/api/v1/feeds/$FEED_TOKEN"
```

## Cost Estimates

- **Fly.io**: ~$5/mo (shared CPU, 512MB RAM, 1GB Postgres)
- **Anthropic**: ~$0.01–$1.50 per article (Sonnet for articles, larger for papers)
- **OpenAI TTS**: ~$0.015 per 1K characters → ~$0.30 for a 20K-char article
- **ElevenLabs**: Depends on plan; Flash v2.5 is cheapest
- **Tigris storage**: Generous free tier; audio files are small (~10–30MB each)
- **Vercel**: Free tier covers the frontend

Total: roughly $5/mo base + $0.30–$2 per episode.

## Monitoring

```bash
fly logs        # Stream backend logs
fly status      # Check app health
fly ssh console # SSH into the VM
```

## Scaling Notes

- The worker and API run in the same process. For heavy usage, scale horizontally with `fly scale count 2` — the `FOR UPDATE SKIP LOCKED` job claiming handles concurrent workers safely.
- Auto-stop/auto-start is enabled, so the machine sleeps when idle and wakes on requests.
