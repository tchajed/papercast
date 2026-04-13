-- Track word count after cleaning and TTS chunk progress
ALTER TABLE episodes ADD COLUMN word_count INTEGER;
ALTER TABLE episodes ADD COLUMN tts_chunks_done INTEGER NOT NULL DEFAULT 0;
ALTER TABLE episodes ADD COLUMN tts_chunks_total INTEGER NOT NULL DEFAULT 0;
