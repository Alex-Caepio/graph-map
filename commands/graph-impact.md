---
name: graph-impact
description: Map uncommitted work, a branch, commit, A..B range, pull request, or agent run against the code graph and compute transitive test reach.
argument-hint: "[work | uncommitted | commit:<sha> | <from>..<to> | run:<id>]"
---

Analyze change impact and test reach for `$ARGUMENTS` (default target: `work`).

**Important**: Always pass `project: "<absolute-repository-root>"` to `graph_*` MCP tools.

1. Verify graph readiness via `graph_status` (`project: "<repo-root>"`, `check: true`); run `graph_build` if `state` is `"off"` and `fallback_graph` is absent.
2. Run `graph_change_impact` with `{ "project": "<repo-root>", "target": "<target-or-work>" }` (supports `commit:<sha>`, `pr:<num>`, `ref:<name>`, `range:<from>..<to>`, or `from`/`to`).
3. For key modified files, call `graph_file_impact` and `graph_memory_links` to surface callers/callees and Project Brain / Memory Bank references.
4. Run `graph_test_reach` with `{ "project": "<repo-root>", "target": "<target-or-work>" }`.
5. If `$ARGUMENTS` names `run:<id>` or mentions agent runs, call `graph_run_audit`.
6. Present changed files, affected callers/callees, linked memory notes, and prioritized test paths cleanly.
