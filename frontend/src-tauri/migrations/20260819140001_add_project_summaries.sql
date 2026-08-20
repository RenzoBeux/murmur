-- The cross-meeting brief for a project: one stored narrative built from the
-- meetings filed under it, regenerated on demand rather than kept live.
--
-- Not a row in summary_processes: that table's PRIMARY KEY *is* meeting_id and
-- it carries an FK to meetings, so a project has nowhere to sit there. Reusing
-- it would mean inventing a meeting.
CREATE TABLE IF NOT EXISTS project_summaries (
    project_id TEXT PRIMARY KEY,
    -- Same vocabulary summary_processes uses, so the frontend's existing status
    -- handling carries over unchanged: 'PENDING' | 'completed' | 'failed' |
    -- 'cancelled'. ('idle' is never stored -- it is the absence of a row.)
    status TEXT NOT NULL,
    -- Same JSON envelope as summary_processes.result -- {"markdown": "..."} --
    -- so mcp::tools::parse_summary / render_summary and
    -- export::markdown::build_summary_markdown read it with no special case.
    result TEXT,
    error TEXT,
    -- Regeneration overwrites in place, so the previous good brief is parked
    -- here for the duration of a run and restored on failure, cancellation, or
    -- an app restart that stranded the row. Without it a failed regenerate
    -- leaves the project with nothing, which is worse than a stale brief.
    -- Mirrors summary_processes.result_backup.
    result_backup TEXT,
    -- Which meetings the STORED result actually covers: a JSON array of
    -- {"id","title","createdAt","source","fingerprint"} written at completion.
    -- Persisted rather than recomputed on read because it must describe the
    -- brief the user is looking at, not the project as it stands now -- that gap
    -- is precisely what "3 meetings added since" is made of. Titles are
    -- snapshots: a meeting later removed from the project cannot have its title
    -- resolved by a join, and that is exactly the row the coverage UI must show.
    covered_meetings TEXT,
    -- stable_text_fingerprint over the covered ids and their per-meeting
    -- fingerprints, in order. One string compare answers "has anything changed
    -- at all", so the common case skips building the diff lists.
    coverage_fingerprint TEXT,
    -- Coarse progress for the polling UI: 'collecting' | 'mapping' | 'reducing'
    -- | 'synthesizing'. A project brief can be several LLM calls, not one, so a
    -- spinner with no movement for minutes is indistinguishable from a hang.
    stage TEXT,
    stage_current INTEGER NOT NULL DEFAULT 0,
    stage_total INTEGER NOT NULL DEFAULT 0,
    -- The language the stored brief was written in. A project has no
    -- folder_path, so the per-meeting metadata.json mechanism has nowhere to
    -- live here; this column is that setting's only home.
    output_language TEXT,
    -- Recorded so the UI can say which model wrote the brief it is showing;
    -- provider and model come from the frontend per run, as they do for meeting
    -- summaries, and are not settings.
    model_provider TEXT,
    model_name TEXT,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    start_time TIMESTAMP,
    end_time TIMESTAMP,
    processing_time REAL NOT NULL DEFAULT 0.0,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

-- No secondary index: every access is by the primary key.

-- Not indexed in search_index either, for the reason spelled out in
-- 20260819140000 -- plus one of its own: this text is derived from per-meeting
-- summaries that are already indexed (source tag 2), so indexing it would mostly
-- return second copies of hits the user already gets, pointing at a surface that
-- has no per-meeting anchor to scroll to.
