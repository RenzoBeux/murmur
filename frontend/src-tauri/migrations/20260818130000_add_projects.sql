-- Projects: a named folder that groups meetings ("Client X", "Weekly sync").
-- A meeting belongs to at most one project (folder semantics — meeting_tags
-- already covers free-form multi-label grouping), so membership is a column on
-- `meetings` rather than a join table.
CREATE TABLE IF NOT EXISTS projects (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

-- Listing sorts by name; the index keeps that ordering cheap as projects grow.
CREATE INDEX IF NOT EXISTS idx_projects_name ON projects(name COLLATE NOCASE);

-- NULL = unfiled. ON DELETE SET NULL means deleting a project unfiles its
-- meetings instead of destroying them; ProjectsRepository::delete does the same
-- unfiling explicitly, so the behaviour holds even with foreign_keys=OFF.
ALTER TABLE meetings ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;

-- The project view filters meetings by project_id (plus deleted_at IS NULL).
CREATE INDEX IF NOT EXISTS idx_meetings_project_id ON meetings(project_id);
