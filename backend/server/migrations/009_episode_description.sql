-- Add per-episode description (generated from transcript) and a
-- 'describe' job type that produces it.
ALTER TABLE episodes ADD COLUMN description TEXT;

CREATE TABLE jobs_new (
    id          TEXT PRIMARY KEY,
    episode_id  TEXT NOT NULL REFERENCES episodes(id) ON DELETE CASCADE,
    job_type    TEXT NOT NULL CHECK (job_type IN ('scrape', 'pdf', 'clean', 'summarize', 'tts', 'image', 'describe')),
    status      TEXT NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'running', 'done', 'error')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    run_after   TEXT NOT NULL DEFAULT (datetime('now')),
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT INTO jobs_new SELECT * FROM jobs;
DROP TABLE jobs;
ALTER TABLE jobs_new RENAME TO jobs;

CREATE INDEX idx_jobs_queued ON jobs(status, run_after)
    WHERE status = 'queued';

-- Backfill description for existing done episodes so the RSS feed has
-- something useful to show while they wait for their describe job.
INSERT INTO jobs (id, episode_id, job_type, status)
SELECT
    lower(hex(randomblob(8))),
    id,
    'describe',
    'queued'
FROM episodes
WHERE status = 'done' AND description IS NULL;
