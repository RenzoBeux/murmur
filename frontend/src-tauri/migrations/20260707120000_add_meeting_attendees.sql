-- Per-meeting attendee roster (free text, user-entered).
-- Injected into summary prompts as the canonical list of participant names so
-- the LLM can correct STT-misheard names and avoid inventing speakers.
ALTER TABLE meetings ADD COLUMN attendees TEXT;
