-- Per-meeting chat history for the in-app Q&A chat feature.
-- One row per message; ordering by created_at ASC reconstructs the conversation.
CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL,
    role TEXT NOT NULL CHECK(role IN ('user','assistant')),
    content TEXT NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (meeting_id) REFERENCES meetings(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_chat_messages_meeting_created
    ON chat_messages(meeting_id, created_at);
