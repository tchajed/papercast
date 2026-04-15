-- Per-call AI usage rows for cost accounting. stage names:
--   'clean', 'summarize', 'describe', 'pdf_extract', 'visual_summary',
--   'image', 'feed_image', 'tts'
-- provider: 'claude', 'gemini', 'google_tts'
-- For TTS, input_tokens is the character count spoken.
CREATE TABLE ai_usage (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    episode_id    TEXT REFERENCES episodes(id) ON DELETE CASCADE,
    feed_id       TEXT REFERENCES feeds(id) ON DELETE SET NULL,
    stage         TEXT NOT NULL,
    provider      TEXT NOT NULL,
    model         TEXT NOT NULL,
    input_tokens  INTEGER NOT NULL DEFAULT 0,
    output_tokens INTEGER NOT NULL DEFAULT 0,
    created_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_ai_usage_episode ON ai_usage(episode_id);
CREATE INDEX idx_ai_usage_created ON ai_usage(created_at);
