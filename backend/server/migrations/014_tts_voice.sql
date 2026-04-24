-- Per-episode Google TTS voice override (e.g. 'en-US-Chirp3-HD-Puck').
-- NULL means fall back to the GOOGLE_TTS_VOICE env var at synthesis time.
ALTER TABLE episodes ADD COLUMN tts_voice TEXT;
