# Meetily MCP Server

A small, **read-only** [Model Context Protocol](https://modelcontextprotocol.io)
server that lets any MCP-capable AI tool on your machine — Claude Desktop,
Cursor, Cline, Windsurf, Claude Code, etc. — read your Meetily meeting
transcripts and summaries.

It reads Meetily's local SQLite database (`meeting_minutes.db`) **directly and
read-only**, so:

- It works even when the Meetily FastAPI backend is **not** running.
- It can never modify or corrupt your meeting data (the DB is opened with
  SQLite `mode=ro`).
- No extra ports or servers — the AI client launches it on demand over stdio.

## Tools exposed

| Tool | Description |
|------|-------------|
| `list_meetings(limit=50)` | List meetings (most recent first) with their `meeting_id`, title, date, and whether a summary exists. |
| `get_transcript(meeting_id)` | Full transcript, with speaker tags and timestamps when available. |
| `get_summary(meeting_id)` | The AI-generated structured summary (people, session summary, decisions, action items, next steps…). |
| `get_meeting(meeting_id)` | Everything at once: summary + full transcript. |
| `search_transcripts(query, limit=20)` | Search across all transcripts; returns matches with context snippets. |

It also exposes two resources for clients that browse resources instead of
calling tools: `meetily://meetings` and `meetily://meeting/{meeting_id}`.

## Setup

Install the one dependency (into the same environment you'll run it from — the
existing `backend/venv` works):

```powershell
# from backend/
venv\Scripts\pip install -r mcp_server\requirements.txt
```

Verify it runs (Ctrl+C to stop — it waits for a client on stdin):

```powershell
venv\Scripts\python mcp_server\server.py
```

## Connecting an AI client (generic stdio config)

Almost every MCP client uses the same JSON shape. Add an entry to your client's
MCP config pointing at the venv Python and this server script:

```jsonc
{
  "mcpServers": {
    "meetily": {
      "command": "C:\\Users\\renzi\\Documents\\Proyectos\\meetily\\backend\\venv\\Scripts\\python.exe",
      "args": [
        "C:\\Users\\renzi\\Documents\\Proyectos\\meetily\\backend\\mcp_server\\server.py"
      ],
      "env": {
        "PYTHONUTF8": "1"
      }
    }
  }
}
```

Where that config lives depends on the client — a few common ones:

- **Claude Desktop**: `%APPDATA%\Claude\claude_desktop_config.json`
- **Cursor**: `.cursor/mcp.json` (project) or `%USERPROFILE%\.cursor\mcp.json` (global)
- **Cline / Roo (VS Code)**: the extension's *MCP Servers* settings
- **Claude Code**: `claude mcp add meetily -- "<python.exe>" "<server.py>"`,
  or a `mcpServers` block in `.mcp.json`

After adding it, restart the client. You should see the `meetily` server connect
and its five tools become available. Try asking: *"List my Meetily meetings"* or
*"Summarize my last meeting."*

## Pointing at a different database

By default the server reads `backend/meeting_minutes.db`. To use a different DB
(e.g. Meetily's production location, `%APPDATA%\Meetily\...`), set
`DATABASE_PATH` in the config's `env` block:

```jsonc
"env": {
  "PYTHONUTF8": "1",
  "DATABASE_PATH": "C:\\Users\\renzi\\AppData\\Roaming\\Meetily\\meeting_minutes.db"
}
```

This is the same `DATABASE_PATH` variable the Meetily backend itself uses.
