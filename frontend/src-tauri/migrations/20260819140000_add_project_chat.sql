-- Project-level AI chat: one conversation about EVERY meeting filed under a
-- project, as opposed to chat_threads/chat_messages, which are scoped to a
-- single meeting.
--
-- Why new tables instead of widening the meeting chat tables:
--   * chat_threads.meeting_id and chat_messages.meeting_id are TEXT NOT NULL
--     with an FK to meetings, and foreign keys are enforced, so a project-scoped
--     row has no legal value to put there. A placeholder meeting row is worse:
--     it would surface in the meeting lists, in search, in exports and in the
--     duration aggregate.
--   * Dropping that NOT NULL needs a table rebuild, and a rebuild is forbidden
--     here -- the FTS5 triggers in 20260713130000 address chat rows by raw rowid
--     ((rowid << 3) | 4). A rebuild drops the triggers and renumbers rowids,
--     silently desyncing search.
-- The cost is two near-duplicate schemas; the benefit is that the meeting chat,
-- which people already use, is not migrated at all.
CREATE TABLE IF NOT EXISTS project_chat_threads (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    title TEXT NOT NULL,
    -- No 'live' origin: a recording belongs to a meeting, never to a project.
    -- The column exists so the row shape matches chat_threads and one Rust
    -- conversion can serve both; the CHECK keeps 'live' out.
    origin TEXT NOT NULL DEFAULT 'post' CHECK(origin IN ('post')),
    -- Same three modes as the meeting chat, same default; see 20260819130000.
    grounding_mode TEXT NOT NULL DEFAULT 'transcript_only',
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- Deleting a project deletes its conversations. Note this differs from what
    -- happens to its meetings: ProjectsRepository::delete unfiles those
    -- (project_id -> NULL) because a meeting outlives its folder. A chat *about*
    -- the folder does not.
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_project_chat_threads_project
    ON project_chat_threads(project_id, created_at);

CREATE TABLE IF NOT EXISTS project_chat_messages (
    id TEXT PRIMARY KEY,
    -- Denormalized from the thread, exactly as chat_messages.meeting_id is:
    -- it makes "wipe every conversation in this project" one statement and
    -- keeps the two chat schemas readable side by side.
    project_id TEXT NOT NULL,
    -- NOT NULL here, unlike chat_messages.thread_id, which is nullable only
    -- because it had to arrive by ALTER TABLE. A fresh table has no such excuse.
    thread_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user','assistant')),
    content TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    -- Grounding outcome and web citations as JSON; same shape as
    -- chat_messages.metadata (ChatAnswerMetadata in api/chat_common.rs).
    metadata TEXT,
    FOREIGN KEY (project_id) REFERENCES projects(id) ON DELETE CASCADE,
    FOREIGN KEY (thread_id) REFERENCES project_chat_threads(id) ON DELETE CASCADE
);

-- Reading a conversation is always thread-scoped, ordered
-- `created_at ASC, rowid ASC`: created_at has second resolution, so the rowid
-- tiebreaker settles same-second inserts. Same rule (and same reason) as
-- chat_messages; there is a regression test for it on the meeting side.
CREATE INDEX IF NOT EXISTS idx_project_chat_messages_thread_created
    ON project_chat_messages(thread_id, created_at);

-- ---------------------------------------------------------------------------
-- DELIBERATELY NOT INDEXED in the FTS5 search_index (20260713130000).
--
-- Not an oversight, and not "for later": as the index is queried today, an entry
-- here would be dead weight. Every consumer
-- (TranscriptsRepository::search_transcripts, mcp::tools::fts_search_rows) runs
-- `JOIN meetings m ON m.id = search_index.meeting_id ... AND m.deleted_at IS
-- NULL` and returns (meeting_id, meeting title, snippet) -- the result type IS a
-- meeting. A project-chat row has no meeting to join to, so the inner join would
-- silently discard every hit while the index kept growing. Worse, project delete
-- would leave orphaned index rows that no sweep can reach: the safety-net
-- trigger trg_si_meetings_ad fires on meetings, and there is no project analogue.
--
-- Making it work later means all of: a LEFT JOIN guarded by
-- `(search_index.meeting_id IS NULL OR m.deleted_at IS NULL)`; a kind
-- discriminator on the search result so the UI knows to open a project chat
-- instead of a meeting; a project_id column on search_index (its schema is fixed
-- at CREATE, so that is a new virtual table plus a full reindex); and a new
-- source tag -- tag 5 is free, the encoding has 8 slots.
-- Until all of that exists, indexing here buys nothing.
-- ---------------------------------------------------------------------------
