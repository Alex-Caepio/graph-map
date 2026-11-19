---
name: graph-impact
description: Analyze code-change blast radius, direct callers/callees, transitive test reach, and AI coding run coverage before committing or reviewing.
---

# Change Impact & Test Reach

Use this skill when assessing uncommitted work, staged diffs, branches, commits, pull requests, or Claude/Codex coding runs against the dependency graph.

## Important Codex & Claude Rule

Always pass `project: "<absolute-repository-root>"` explicitly to every `graph_*` MCP tool call (in Codex, the MCP server starts with `cwd` inside the plugin cache).

## Procedure

1. **Ensure graph availability**:
   Call `graph_status` with `{ "project": "<repo-root>", "check": true }`.
   If `state` is `"off"` and `fallback_graph` is absent, run `graph_build` with `{ "project": "<repo-root>" }` (or `{ "project": "<repo-root>", "import": "<path/to/graph.json>" }`).
2. **Map change impact**:
   Call `graph_change_impact` with `{ "project": "<repo-root>", "target": "work" }` (or `uncommitted`, `commit:<sha>`, `branch:<name>`, `pull:<num>`, `run:<session-id>`).
   - Set `certain: true` to restrict edges to statically verified code references (omitting name-guessed links).
   - Add `base: "<commit-sha>"` for stacked pull-request comparison.
3. **Compute transitive test reach**:
   Call `graph_test_reach` with `{ "project": "<repo-root>", "target": "work" }` to rank the shortest incoming paths from test files to every modified source file.
4. **Audit agent run coverage (optional)**:
   Call `graph_run_audit` with `{ "project": "<repo-root>" }` to list recorded Claude Code, Codex, and Harness runs, then pass `session: "<id>"` to compare edited files against dependent files that were never opened or shell-read.
5. **Drill into suspicious edges**:
   Use `graph_link` (`from`, `to`) or `graph_neighbors` (`node`) on high-fan-in dependents.

## Output Checklist

- Summarize changed files and affected callers/callees from `graph_change_impact`.
- List prioritized test paths from `graph_test_reach`.
- Flag high-impact dependents not opened during a recorded `run:<id>` audit, if applicable.
