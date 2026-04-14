# Tigris public-access investigation (2026-04-14)

## Symptoms
- Browser: Chrome shows `BLOCKED_BY_ORB` / 403 on `<audio>` elements.
- Overcast: "blocked by publisher" on all episodes.
- `curl` on `https://tchajed-podcast-audio.fly.storage.tigris.dev/…` returned
  403 AccessDenied with an XML body — but HEAD returned 200, which threw
  early debugging off the scent.

## What we tried (in order)
1. **Added `.acl(public-read)` to uploads** in `pipeline/storage.rs`.
   Didn't help — Tigris stored the ACL (visible via `get_object_acl`) but
   didn't act on it.
2. **Ran `put_bucket_policy`** via AWS CLI → `NotImplemented`. Tigris
   doesn't implement bucket policies over the S3 API.
3. **Tried `AWS_REQUEST_CHECKSUM_CALCULATION=when_required`** env var to
   disable the AWS CLI v2.23+ checksum headers. Still `NotImplemented`.
4. **Switched to boto3** — same `NotImplemented`.
5. **Ran `put_bucket_acl public-read`** → accepted, and `get_bucket_acl`
   returned `AllUsers: READ` grants. Still 403 on GET.
6. **`fly storage update --public`** flipped the metadata flag to
   `Public: True`. Still 403.
7. **Uploaded a fresh object with ACL=public-read** → still 403. So it
   wasn't a per-object staleness from pre-public uploads.
8. **Tried hostname `t3.storage.dev`** (guessed from partial info). Got
   intermittent 200s, then 403s. Likely not a real public hostname.
9. **Tigris support confirmed** the real public hostname is
   `https://<bucket>.t3.tigrisfiles.io`. Objects return 200 reliably
   there, including with full Chrome-style headers (Referer, Range,
   Sec-Fetch-*, Origin).

## Current state
- Uploads in `pipeline/storage.rs` return `https://<bucket>.t3.tigrisfiles.io/<key>`.
- Migration 006 (already applied on prod) rewrote URLs to `t3.storage.dev`
  (wrong).
- Migration 007 rewrites `t3.storage.dev` and any residual
  `fly.storage.tigris.dev` → `t3.tigrisfiles.io`.
- `delete_object` accepts any of the three historical hostnames.
- `.acl(public-read)` on uploads is kept as belt-and-suspenders — it may
  be a no-op at Tigris, but it's harmless.

## Why the confidence is low
- We never got a clean explanation for why `fly.storage.tigris.dev`
  returned 403 even after `fly storage update --public`. If that was
  purely edge-cached negative responses, we'd expect cache-busting query
  strings to work — they didn't. But we also saw intermittent 200s during
  the same session, which suggests cache heterogeneity across edges
  rather than a true auth issue.
- The relationship between `--public`, S3 bucket ACL, per-object ACL,
  and the "correct" public hostname is undocumented from our POV.
- `fly.storage.tigris.dev` might still be a private/internal hostname
  while `t3.tigrisfiles.io` is the CDN-fronted public one. In that case
  our uploads should probably continue using `fly.storage.tigris.dev` as
  the S3 *endpoint* (they do — `AWS_ENDPOINT_URL_S3` is unchanged), but
  publish URLs on `t3.tigrisfiles.io`.

## Things to verify later
- After the 007 migration lands and the app is healthy, hard-refresh an
  episode page in Chrome and force-refresh Overcast; confirm both work.
- If anything regresses, signed URLs (presigned GETs from the backend)
  are a clean fallback that sidesteps public-access entirely.
- Consider centralizing the public-hostname as a config value
  (`TIGRIS_PUBLIC_HOST` or similar) instead of hardcoding, so a future
  rename doesn't require a code change.
- The per-object `put_object_acl` backfill we ran during debugging was
  probably unnecessary once the hostname was right; it didn't hurt but
  also may not have been the reason for eventual success.

## Files touched
- `backend/server/src/pipeline/storage.rs` — hostname + ACL.
- `backend/server/migrations/006_t3_hostname.sql` — initial (wrong)
  rewrite to `t3.storage.dev`. Do **not** edit; sqlx checksums it.
- `backend/server/migrations/007_t3_hostname_fix.sql` — corrective
  rewrite to `t3.tigrisfiles.io`.
