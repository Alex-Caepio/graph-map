---
name: trace-scenario
description: Trace execution scenarios, shortest call paths, and step-by-step neighbor relationships across a codebase with graph-map.
---

# Trace Scenario & Dependency Paths

Use this skill to understand how an entry point, controller, command, or domain service flows through the repository.

## Important Codex & Claude Rule

Always pass `project: "<absolute-repository-root>"` explicitly to every `graph_*` MCP tool call.

## Procedure

1. **Locate exact targets**:
   Call `graph_find` with `{ "project": "<repo-root>", "query": "<term>" }` to confirm exact file paths or symbol keys.
2. **Trace forward scenario**:
   Call `graph_scenario` with `{ "project": "<repo-root>", "entry": "<file-or-symbol>", "notests": true }`.
   Use `skip` if paginated results contain many downstream hops.
3. **Inspect shortest paths and detailed edges**:
   - Call `graph_path` with `from` and `to` to extract the shortest directed dependency route between two components.
   - Call `graph_link` with `from` and `to` to view symbol-to-symbol call edges, line numbers, and link confidence (`certain: true` for verified edges only).
   - Call `graph_neighbors` with `node`, plus `out` / `into` arrays, to unfold upstream callers or downstream callees step by step.

## Output Checklist

- Present verified call/dependency chains using exact file paths and symbols observed in tool outputs.
- Clearly distinguish statically extracted links (`certain: true`) from heuristic/name-guessed links.
