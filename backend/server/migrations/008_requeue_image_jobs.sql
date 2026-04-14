-- Requeue image jobs that failed under the old (wrong) Gemini model.
-- Image failures are non-fatal so these were harmless, but now that the
-- model is fixed we want them to run.
UPDATE jobs
SET status = 'queued', attempts = 0, run_after = datetime('now')
WHERE job_type = 'image' AND status = 'error';
