-- Multiple chat conversations (threads) per meeting. The live-recording chat
-- becomes an origin='live' thread; the Chat tab creates origin='post' threads.
CREATE TABLE IF NOT EXISTS chat_threads (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    title TEXT NOT NULL,
    origin TEXT NOT NULL DEFAULT 'post' CHECK(origin IN ('live','post')),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_chat_threads_meeting ON chat_threads(meeting_id, created_at);

-- MUST be ADD COLUMN, never a table rebuild: the FTS5 search_index triggers
-- (20260713130000) address chat rows by raw rowid ((rowid << 3) | 4); a rebuild
-- would drop those triggers and renumber rowids, silently desyncing search.
ALTER TABLE chat_messages ADD COLUMN thread_id TEXT REFERENCES chat_threads(id) ON DELETE CASCADE;

-- Backfill: each meeting's existing messages become one default thread,
-- anchored at its first message's timestamp. Deterministic thread id.
INSERT INTO chat_threads (id, meeting_id, title, origin, created_at)
SELECT 'thread-' || meeting_id, meeting_id, 'Chat 1', 'post', MIN(created_at)
FROM chat_messages GROUP BY meeting_id;

UPDATE chat_messages SET thread_id = 'thread-' || meeting_id WHERE thread_id IS NULL;

CREATE INDEX IF NOT EXISTS idx_chat_messages_thread_created
    ON chat_messages(thread_id, created_at);
