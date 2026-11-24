---
name: commit-work-map
description: Inspect commits, pull requests, branch diffs, and Claude/Codex agent runs on the code graph, or open the interactive localhost Graph Map UI.
---

# Commit, Pull Request & Run Mapping

Use this skill to explore git history targets, branch diffs, agent session runs, or launch the interactive WASM browser map.

## Important Codex & Claude Rule

Always pass `project: "<absolute-repository-root>"` explicitly to every `graph_*` MCP tool call.

## Procedure

1. **Inspect runs or commits**:
   - Call `graph_run_audit` with `{ "project": "<repo-root>" }` to list recent Claude Code (`~/.claude/projects/`), Codex (`~/.codex/sessions/`), and `.graph-map/runs/` activity logs.
   - Pass `session: "<id>"` (and optionally `launch` / `compare_launch`) to audit edited files vs unopened dependent files and verified `Shell` read evidence.
   - Call `graph_history_hotspots` with `{ "project": "<repo-root>", "branch": "<name>", "query": "<filter>" }` to inspect commit history and co-change hotspots.
2. **Map a specific target or commit range**:
   Call `graph_change_impact` (or `graph_file_impact` for one file) with `target` set to `work`, `uncommitted`, `commit:<sha>`, `ref:<name>`, `pr:<number>`, `range:<from>..<to>`, or `run:<session-id>` (or pass `from` and `to` directly for `A..B` range comparison).
3. **Open interactive Web UI (when visual exploration is helpful)**:
   Run in a terminal:
   ```bash
   graph-map --project "<repo-root>" serve --host 127.0.0.1 --port 7840
   ```
   and share the printed `http://127.0.0.1:<port>` URL with the user (Canvas2D dependency map across 5 lenses, interactive commit DAG browser, and live SSE status updates).
