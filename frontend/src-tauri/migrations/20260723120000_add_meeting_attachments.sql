-- Files attached to a meeting (screenshots, whiteboard photos, docs). The file
-- bytes live at {app_data_dir}/attachments/{meeting_id}/{stored_name}; the DB
-- stores only the relative stored_name so the database stays relocatable.
-- Soft-deleting a meeting leaves rows and files intact (restorable); hard purge
-- deletes rows (manual cascade + FK) and best-effort removes the folder.
CREATE TABLE IF NOT EXISTS meeting_attachments (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    file_name TEXT NOT NULL,
    stored_name TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_meeting_attachments_meeting ON meeting_attachments(meeting_id);
