-- 006 migrated to t3.storage.dev, but the correct public Tigris hostname is
-- t3.tigrisfiles.io. Also catch any lingering fly.storage.tigris.dev URLs.
UPDATE episodes
SET audio_url = REPLACE(audio_url, '.t3.storage.dev/', '.t3.tigrisfiles.io/')
WHERE audio_url LIKE '%.t3.storage.dev/%';

UPDATE episodes
SET image_url = REPLACE(image_url, '.t3.storage.dev/', '.t3.tigrisfiles.io/')
WHERE image_url LIKE '%.t3.storage.dev/%';

UPDATE episodes
SET audio_url = REPLACE(audio_url, '.fly.storage.tigris.dev/', '.t3.tigrisfiles.io/')
WHERE audio_url LIKE '%.fly.storage.tigris.dev/%';

UPDATE episodes
SET image_url = REPLACE(image_url, '.fly.storage.tigris.dev/', '.t3.tigrisfiles.io/')
WHERE image_url LIKE '%.fly.storage.tigris.dev/%';
