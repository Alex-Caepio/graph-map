---
name: graph-impact
description: Map uncommitted work, a branch, commit, pull request, or agent run against the code graph and compute transitive test reach.
---

Analyze change impact and test reach for `$ARGUMENTS` (default target: `work`).

**Important**: Always pass `project: "<absolute-repository-root>"` to `graph_*` MCP tools.

1. Verify graph readiness via `graph_status` (`project: "<repo-root>"`, `check: true`).
2. Run `graph_change_impact` with `{ "project": "<repo-root>", "target": "<target-or-work>" }`.
3. Run `graph_test_reach` with `{ "project": "<repo-root>", "target": "<target-or-work>" }`.
4. If `$ARGUMENTS` names `run:<id>` or mentions agent runs, call `graph_run_audit`.
5. Present changed files, affected callers/callees, and prioritized test paths cleanly.
