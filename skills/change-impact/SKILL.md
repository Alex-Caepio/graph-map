---
name: change-impact
description: Analyze code-change blast radius, direct callers/callees, symbol diffs, transitive test reach, linked project memory, and AI coding run coverage before committing or reviewing.
---

# Change Impact & Test Reach

Use this skill when assessing uncommitted work, staged diffs, branches, commits, commit ranges (`A..B`), pull requests, or Claude/Codex coding runs against the dependency graph.

## Important Codex & Claude Rule

Always pass `project: "<absolute-repository-root>"` explicitly to every `graph_*` MCP tool call (in Codex, the MCP server starts with `cwd` inside the plugin cache).

## Procedure

1. **Ensure graph availability**:
   Call `graph_status` with `{ "project": "<repo-root>", "check": true }`.
   If `state` is `"off"` and `fallback_graph` is absent, run `graph_build` with `{ "project": "<repo-root>" }` (builds the native 13-language tree-sitter AST + SQLite IR automatically; or pass `import: "<path/to/graph.json>"`).
2. **Map change impact (targets & `A..B` ranges)**:
   Call `graph_change_impact` with `{ "project": "<repo-root>", "target": "work" }` (or `uncommitted`, `commit:<sha>`, `ref:<name>`, `pr:<num>`, `range:<from>..<to>`, `run:<session-id>`, or explicit `from` + `to` range params).
   - Pass `lod` (`"overview"`, `"folder"`, `"file"`, `"symbol"`) and `min_weight` to control progressive detail on large diffs, or `catalog_only: true` for a compact file/symbol diff summary.
   - Pass `focus_session: "<session-id>"` to overlay live prompt/edited targets, `certain: true` for statically extracted links only, and `base: "<sha-or-ref>"` for stacked PR / range comparisons.
3. **Inspect per-file blast & linked memory**:
   - Call `graph_file_impact` with `{ "project": "<repo-root>", "path": "<file>", "target": "<target>" }` for exact callers, callees, symbol diffs, and reaching tests for a hot file.
   - Call `graph_memory_links` with `{ "project": "<repo-root>", "path": "<file>" }` (or `files: [...]`) to check Project Brain findings/decisions and Memory Bank `MEM-*.md` chunks citing that file.
4. **Compute transitive test reach**:
   Call `graph_test_reach` with `{ "project": "<repo-root>", "target": "work" }` (or `from`/`to`) to rank the shortest incoming paths from test files to every modified source file.
5. **Audit agent run coverage (optional)**:
   Call `graph_run_audit` with `{ "project": "<repo-root>" }` to list recorded Claude Code, Codex, and Harness runs, then pass `session: "<id>"` to compare edited files against dependent files that were never opened or shell-read.
6. **Drill into suspicious edges**:
   Use `graph_link` (`from`, `to`) or `graph_neighbors` (`node`, optional `depth: 1..6`) on high-fan-in dependents.

## Output Checklist

- Summarize changed files, symbol-level blast, and affected callers/callees from `graph_change_impact` / `graph_file_impact`.
- Note any active decisions, warnings, or stale citations from `graph_memory_links`.
- List prioritized test paths from `graph_test_reach`.
- Flag high-impact dependents not opened during a recorded `run:<id>` audit, if applicable.
