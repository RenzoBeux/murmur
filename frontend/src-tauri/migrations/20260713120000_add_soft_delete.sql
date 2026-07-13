-- Soft-delete (trash) support for meetings.
--
-- A non-NULL `deleted_at` marks a meeting as trashed: it disappears from every
-- listing and search surface, but its child rows (transcripts, summaries,
-- transcript_chunks) are left completely intact so the delete can be undone.
-- Trashed meetings are hard-purged (cascading to their children) after a
-- retention window, best-effort at startup. See MeetingsRepository::delete_meeting
-- / restore_meeting / purge_meeting / purge_trash_older_than.
--
-- Stored as TEXT (the same encoding sqlx uses for the other timestamp columns),
-- so lexicographic comparison against a cutoff is a valid ordering.
ALTER TABLE meetings ADD COLUMN deleted_at TEXT;

-- Speeds up the "still-live meetings" filter (deleted_at IS NULL) that every
-- listing/search query now carries, and the trash-purge scan (deleted_at < cutoff).
CREATE INDEX IF NOT EXISTS idx_meetings_deleted_at ON meetings(deleted_at);
