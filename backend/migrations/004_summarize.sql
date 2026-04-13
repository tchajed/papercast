-- Recreate episodes table to add summarize flag, transcript column,
-- and 'summarizing' status value.
CREATE TABLE episodes_new (
    id             TEXT PRIMARY KEY,
    feed_id        TEXT NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    title          TEXT NOT NULL DEFAULT '',
    source_url     TEXT,
    source_type    TEXT NOT NULL CHECK (source_type IN ('article', 'arxiv', 'pdf')),
    raw_text       TEXT,
    cleaned_text   TEXT,
    transcript     TEXT,
    audio_url      TEXT,
    image_url      TEXT,
    duration_secs  INTEGER,
    tts_provider   TEXT CHECK (tts_provider IN ('openai', 'elevenlabs', 'google')),
    status         TEXT NOT NULL DEFAULT 'pending'
                       CHECK (status IN ('pending','scraping','cleaning','summarizing',
                                         'tts','image','done','error')),
    error_msg      TEXT,
    pub_date       TEXT,
    created_at     TEXT NOT NULL DEFAULT (datetime('now')),
    word_count     INTEGER,
    tts_chunks_done  INTEGER NOT NULL DEFAULT 0,
    tts_chunks_total INTEGER NOT NULL DEFAULT 0,
    summarize      INTEGER NOT NULL DEFAULT 0
);

INSERT INTO episodes_new (id, feed_id, title, source_url, source_type, raw_text,
    cleaned_text, audio_url, image_url, duration_secs, tts_provider, status,
    error_msg, pub_date, created_at, word_count, tts_chunks_done, tts_chunks_total)
SELECT id, feed_id, title, source_url, source_type, raw_text,
    cleaned_text, audio_url, image_url, duration_secs, tts_provider, status,
    error_msg, pub_date, created_at, word_count, tts_chunks_done, tts_chunks_total
FROM episodes;

DROP TABLE episodes;
ALTER TABLE episodes_new RENAME TO episodes;

CREATE INDEX idx_episodes_feed_status ON episodes(feed_id, status);
CREATE INDEX idx_episodes_pub_date    ON episodes(feed_id, pub_date DESC);

-- Recreate jobs table to allow 'summarize' job_type
CREATE TABLE jobs_new (
    id          TEXT PRIMARY KEY,
    episode_id  TEXT NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    job_type    TEXT NOT NULL CHECK (job_type IN ('scrape', 'pdf', 'clean', 'summarize', 'tts', 'image')),
    status      TEXT NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'done', 'error')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    run_after   TEXT NOT NULL DEFAULT (datetime('now')),
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO jobs_new SELECT * FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_new RENAME TO jobs;
