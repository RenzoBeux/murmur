"""Meetily MCP server — read-only access to meeting transcripts and summaries.

Exposes Meetily's local SQLite database (``meeting_minutes.db``) over the Model
Context Protocol (stdio transport) so other AI tools on this machine — Claude
Desktop, Cursor, Cline, Windsurf, Claude Code, etc. — can list meetings, read
transcripts, fetch summaries, and search across everything.

Design notes:
  * The database is always opened **read-only** (SQLite ``mode=ro``), so this
    server can never modify or corrupt meeting data.
  * It reads the DB file directly and does **not** require the Meetily FastAPI
    backend to be running.
  * DB location is resolved from ``DATABASE_PATH`` (same env var the backend
    uses); otherwise it defaults to ``meeting_minutes.db`` in the backend dir.

Run it directly for stdio:  ``python server.py``
"""
from __future__ import annotations

import json
import os
import sqlite3
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator, Optional

from mcp.server.fastmcp import FastMCP

mcp = FastMCP("meetily")


# --------------------------------------------------------------------------- #
# Database access (read-only)
# --------------------------------------------------------------------------- #
def resolve_db_path() -> Path:
    """Locate the Meetily SQLite database.

    Honours ``DATABASE_PATH`` (the same variable the FastAPI backend reads);
    otherwise falls back to ``meeting_minutes.db`` in the backend directory
    (the parent of this file's directory).
    """
    env = os.getenv("DATABASE_PATH")
    if env:
        return Path(env).expanduser().resolve()
    return (Path(__file__).resolve().parent.parent / "meeting_minutes.db").resolve()


@contextmanager
def connect() -> Iterator[sqlite3.Connection]:
    """Yield a read-only connection to the Meetily database."""
    path = resolve_db_path()
    if not path.exists():
        raise FileNotFoundError(
            f"Meetily database not found at '{path}'. "
            "Set the DATABASE_PATH environment variable to point at your "
            "meeting_minutes.db file."
        )
    # mode=ro guarantees this process can never write to the DB.
    conn = sqlite3.connect(f"file:{path.as_posix()}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        yield conn
    finally:
        conn.close()


# --------------------------------------------------------------------------- #
# Formatting helpers
# --------------------------------------------------------------------------- #
def _fmt_timestamp(seconds: Optional[float]) -> str:
    """Render recording-relative seconds as ``mm:ss`` (or ``h:mm:ss``)."""
    if seconds is None:
        return ""
    seconds = int(seconds)
    h, rem = divmod(seconds, 3600)
    m, s = divmod(rem, 60)
    return f"{h}:{m:02d}:{s:02d}" if h else f"{m:02d}:{s:02d}"


def _parse_summary(raw: Optional[str]) -> Optional[dict]:
    """Decode a ``summary_processes.result`` blob into a dict.

    The backend has historically stored this value either JSON-encoded once or
    double-encoded (a JSON string containing JSON), so handle both.
    """
    if not raw:
        return None
    try:
        data = json.loads(raw)
        if isinstance(data, str):
            data = json.loads(data)
        return data if isinstance(data, dict) else None
    except (json.JSONDecodeError, TypeError):
        return None


def _render_blocks(blocks: list) -> list[str]:
    """Turn a list of summary ``Block`` dicts into markdown lines."""
    lines: list[str] = []
    for block in blocks or []:
        if not isinstance(block, dict):
            continue
        content = str(block.get("content", "")).strip()
        if not content:
            continue
        btype = block.get("type", "text")
        if btype == "heading1":
            lines.append(f"## {content}")
        elif btype == "heading2":
            lines.append(f"### {content}")
        elif btype == "bullet":
            lines.append(f"- {content}")
        else:
            lines.append(content)
    return lines


def _render_summary(summary: dict) -> str:
    """Render a parsed summary dict as readable markdown.

    Prefers the ordered ``MeetingNotes.sections`` structure (what the Meetily
    UI displays); falls back to the individually named top-level sections.
    """
    parts: list[str] = []
    name = summary.get("MeetingName")
    if name:
        parts.append(f"# {name}")

    notes = summary.get("MeetingNotes")
    sections = notes.get("sections") if isinstance(notes, dict) else None

    if isinstance(sections, list) and sections:
        for section in sections:
            if not isinstance(section, dict):
                continue
            title = str(section.get("title", "")).strip()
            body = _render_blocks(section.get("blocks", []))
            if not body:
                continue
            if title:
                parts.append(f"## {title}")
            parts.extend(body)
    else:
        # Fallback: the named top-level sections.
        section_keys = [
            "People",
            "SessionSummary",
            "CriticalDeadlines",
            "KeyItemsDecisions",
            "ImmediateActionItems",
            "NextSteps",
        ]
        for key in section_keys:
            section = summary.get(key)
            if not isinstance(section, dict):
                continue
            body = _render_blocks(section.get("blocks", []))
            if not body:
                continue
            parts.append(f"## {section.get('title', key)}")
            parts.extend(body)

    return "\n\n".join(parts).strip() or "(Summary is empty.)"


# --------------------------------------------------------------------------- #
# Data access helpers (plain functions, unit-testable without MCP)
# --------------------------------------------------------------------------- #
def fetch_meetings(limit: int = 50) -> list[dict]:
    """Return meetings (most recent first) with a flag for summary availability."""
    with connect() as conn:
        rows = conn.execute(
            """
            SELECT m.id, m.title, m.created_at, m.updated_at,
                   sp.status AS summary_status
            FROM meetings m
            LEFT JOIN summary_processes sp ON sp.meeting_id = m.id
            ORDER BY m.created_at DESC
            LIMIT ?
            """,
            (limit,),
        ).fetchall()
    return [dict(r) for r in rows]


def fetch_transcript(meeting_id: str) -> Optional[dict]:
    """Return meeting metadata plus its transcript segments (or chunk fallback)."""
    with connect() as conn:
        meeting = conn.execute(
            "SELECT id, title, created_at, updated_at FROM meetings WHERE id = ?",
            (meeting_id,),
        ).fetchone()
        if meeting is None:
            return None

        segments = conn.execute(
            """
            SELECT transcript, timestamp, audio_start_time, speaker
            FROM transcripts
            WHERE meeting_id = ?
            ORDER BY audio_start_time IS NULL, audio_start_time, rowid
            """,
            (meeting_id,),
        ).fetchall()

        chunk = None
        if not segments:
            chunk_row = conn.execute(
                "SELECT transcript_text FROM transcript_chunks WHERE meeting_id = ?",
                (meeting_id,),
            ).fetchone()
            chunk = chunk_row["transcript_text"] if chunk_row else None

    return {
        "meeting": dict(meeting),
        "segments": [dict(s) for s in segments],
        "chunk_text": chunk,
    }


def fetch_summary(meeting_id: str) -> Optional[dict]:
    """Return summary status + parsed summary dict for a meeting."""
    with connect() as conn:
        title_row = conn.execute(
            "SELECT title FROM meetings WHERE id = ?", (meeting_id,)
        ).fetchone()
        if title_row is None:
            return None
        row = conn.execute(
            """
            SELECT status, result, error, updated_at
            FROM summary_processes
            WHERE meeting_id = ?
            """,
            (meeting_id,),
        ).fetchone()

    if row is None:
        return {"title": title_row["title"], "status": "none", "summary": None, "error": None}
    return {
        "title": title_row["title"],
        "status": (row["status"] or "unknown").lower(),
        "summary": _parse_summary(row["result"]),
        "error": row["error"],
        "updated_at": row["updated_at"],
    }


def search(query: str, limit: int = 20) -> list[dict]:
    """Case-insensitive substring search across transcript segments and chunks."""
    if not query or not query.strip():
        return []
    like = f"%{query.lower()}%"
    results: list[dict] = []
    seen: set[str] = set()

    def _snippet(text: str) -> str:
        idx = text.lower().find(query.lower())
        if idx < 0:
            return text[:200]
        start = max(0, idx - 100)
        end = min(len(text), idx + len(query) + 100)
        snippet = text[start:end]
        if start > 0:
            snippet = "…" + snippet
        if end < len(text):
            snippet = snippet + "…"
        return snippet

    with connect() as conn:
        seg_rows = conn.execute(
            """
            SELECT m.id, m.title, t.transcript, t.timestamp
            FROM meetings m
            JOIN transcripts t ON t.meeting_id = m.id
            WHERE LOWER(t.transcript) LIKE ?
            ORDER BY m.created_at DESC
            LIMIT ?
            """,
            (like, limit),
        ).fetchall()
        for r in seg_rows:
            results.append(
                {
                    "meeting_id": r["id"],
                    "title": r["title"],
                    "timestamp": r["timestamp"],
                    "context": _snippet(r["transcript"]),
                }
            )
            seen.add(r["id"])

        if len(results) < limit:
            chunk_rows = conn.execute(
                """
                SELECT m.id, m.title, tc.transcript_text
                FROM meetings m
                JOIN transcript_chunks tc ON tc.meeting_id = m.id
                WHERE LOWER(tc.transcript_text) LIKE ?
                ORDER BY m.created_at DESC
                LIMIT ?
                """,
                (like, limit),
            ).fetchall()
            for r in chunk_rows:
                if r["id"] in seen:
                    continue
                results.append(
                    {
                        "meeting_id": r["id"],
                        "title": r["title"],
                        "timestamp": None,
                        "context": _snippet(r["transcript_text"]),
                    }
                )

    return results[:limit]


def _render_transcript(data: dict) -> str:
    """Render fetch_transcript output as readable text."""
    meeting = data["meeting"]
    header = f"# {meeting['title']}\n(meeting_id: {meeting['id']} · created {meeting['created_at']})"

    if data["segments"]:
        lines = []
        for seg in data["segments"]:
            ts = _fmt_timestamp(seg.get("audio_start_time"))
            speaker = seg.get("speaker")
            prefix = ""
            if ts:
                prefix += f"[{ts}] "
            if speaker:
                prefix += f"{speaker}: "
            lines.append(f"{prefix}{seg['transcript']}".strip())
        return header + "\n\n" + "\n".join(lines)

    if data["chunk_text"]:
        return header + "\n\n" + data["chunk_text"]

    return header + "\n\n(No transcript recorded for this meeting.)"


# --------------------------------------------------------------------------- #
# MCP tools
# --------------------------------------------------------------------------- #
@mcp.tool()
def list_meetings(limit: int = 50) -> str:
    """List Meetily meetings, most recent first.

    Args:
        limit: Maximum number of meetings to return (default 50).

    Returns a markdown list of meetings with their ``meeting_id``, title,
    creation date, and whether a summary has been generated. Use the
    ``meeting_id`` values with the other tools.
    """
    meetings = fetch_meetings(limit)
    if not meetings:
        return "No meetings found in the Meetily database yet."
    lines = [f"Found {len(meetings)} meeting(s):", ""]
    for m in meetings:
        has_summary = (m.get("summary_status") or "").lower() == "completed"
        badge = "📝 summary" if has_summary else "—"
        lines.append(f"- **{m['title']}** · `{m['id']}` · {m['created_at']} · {badge}")
    return "\n".join(lines)


@mcp.tool()
def get_transcript(meeting_id: str) -> str:
    """Get the full transcript of a Meetily meeting.

    Args:
        meeting_id: The meeting's id (from ``list_meetings``).

    Returns the transcript with speaker tags and timestamps when available.
    """
    data = fetch_transcript(meeting_id)
    if data is None:
        return f"No meeting found with id '{meeting_id}'. Use list_meetings to see valid ids."
    return _render_transcript(data)


@mcp.tool()
def get_summary(meeting_id: str) -> str:
    """Get the AI-generated summary of a Meetily meeting.

    Args:
        meeting_id: The meeting's id (from ``list_meetings``).

    Returns the structured summary (people, session summary, decisions, action
    items, next steps, …) as markdown, or a status note if no summary exists.
    """
    data = fetch_summary(meeting_id)
    if data is None:
        return f"No meeting found with id '{meeting_id}'. Use list_meetings to see valid ids."

    status = data["status"]
    if status == "none":
        return f"No summary has been generated for '{data['title']}' yet."
    if status in ("processing", "pending", "started"):
        return f"Summary for '{data['title']}' is still being generated (status: {status})."
    if status == "failed":
        return f"Summary generation failed for '{data['title']}': {data.get('error') or 'unknown error'}"
    if not data["summary"]:
        return f"Summary for '{data['title']}' is marked '{status}' but no summary data is available."
    return _render_summary(data["summary"])


@mcp.tool()
def get_meeting(meeting_id: str) -> str:
    """Get everything about a meeting: metadata, summary, and full transcript.

    Args:
        meeting_id: The meeting's id (from ``list_meetings``).

    Convenience tool that combines get_summary and get_transcript in one call.
    """
    summary = get_summary(meeting_id)
    transcript = get_transcript(meeting_id)
    return f"{summary}\n\n---\n\n{transcript}"


@mcp.tool()
def search_transcripts(query: str, limit: int = 20) -> str:
    """Search across all meeting transcripts for a word or phrase.

    Args:
        query: Text to search for (case-insensitive substring match).
        limit: Maximum number of matches to return (default 20).

    Returns matching meetings with a snippet of surrounding context and the
    ``meeting_id`` to retrieve the full transcript or summary.
    """
    if not query or not query.strip():
        return "Please provide a non-empty search query."
    matches = search(query, limit)
    if not matches:
        return f"No transcripts matched '{query}'."
    lines = [f"Found {len(matches)} match(es) for '{query}':", ""]
    for m in matches:
        lines.append(f"- **{m['title']}** · `{m['meeting_id']}`")
        lines.append(f"  > {m['context']}")
    return "\n".join(lines)


# --------------------------------------------------------------------------- #
# MCP resources (for clients that browse resources instead of calling tools)
# --------------------------------------------------------------------------- #
@mcp.resource("meetily://meetings")
def meetings_resource() -> str:
    """A browsable list of all Meetily meetings."""
    return list_meetings(limit=200)


@mcp.resource("meetily://meeting/{meeting_id}")
def meeting_resource(meeting_id: str) -> str:
    """Full details (summary + transcript) for a single meeting."""
    return get_meeting(meeting_id)


if __name__ == "__main__":
    mcp.run()
