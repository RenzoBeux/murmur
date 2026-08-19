-- Chat grounding modes: how far past the transcript the assistant may reach.
--
--   'transcript_only'   strict grounding (the behavior before this migration)
--   'general_knowledge' may answer from the model's own knowledge, no network
--   'web_search'        may also use the provider's server-side web search
--
-- Per-thread rather than global so a strict recap thread and a research thread
-- can sit side by side in the same meeting.
ALTER TABLE chat_threads ADD COLUMN grounding_mode TEXT NOT NULL DEFAULT 'transcript_only';

-- Where an answer came from, plus its web citations, as JSON on the assistant
-- message. Nullable: user messages and every pre-existing row have none.
--
-- MUST be ADD COLUMN, never a table rebuild: the FTS5 search_index triggers
-- (20260713130000) address chat rows by raw rowid ((rowid << 3) | 4); a rebuild
-- would drop those triggers and renumber rowids, silently desyncing search.
ALTER TABLE chat_messages ADD COLUMN metadata TEXT;
