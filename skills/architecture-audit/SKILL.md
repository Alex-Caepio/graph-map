---
name: architecture-audit
description: Audit software architecture layers, cycle regressions, shared coupling, module boundaries, and git co-change hotspots using graph-map.
---

# Architecture, Layers & Cycle Audit

Use this skill to evaluate layering discipline, dependency inversions, circular dependencies, shared bottlenecks, and historical churn hotspots.

## Important Codex & Claude Rule

Always pass `project: "<absolute-repository-root>"` explicitly to every `graph_*` MCP tool call.

## Procedure

1. **Ensure graph availability**:
   Call `graph_status` with `{ "project": "<repo-root>", "check": true }`. Build natively or import if needed via `graph_build`.
2. **Audit layers, patterns, or modules**:
   - Call `graph_architecture` with `{ "project": "<repo-root>" }` to inspect inferred architectural rings/layers and violations, plus `.graph-map/layers` rules if present in the repo.
   - Pass `lod` (`"overview"`, `"folder"`, `"file"`, `"symbol"`) and `min_weight` to slice large codebases progressively, and `no_incoming: true` to surface root/dead-code candidates without incoming callers.
   - Pass `pattern: "layers@"` for direct layer violation focus, or `modules: true` for community module boundaries.
   - Pass `target: "work"` (or `commit:<sha>`, `pr:<num>`, `range:<from>..<to>`, or `from`+`to`) and `base: "<sha-or-ref>"` to check whether a change added or resolved violations.
3. **Trace cyclic dependencies**:
   Call `graph_cycle` with `{ "project": "<repo-root>", "cycle": "<file-or-folder>" }` to find the shortest directed cycle back to the origin.
4. **Inspect shared coupling & hotspots**:
   - Call `graph_shared` with `from`, `to`, and `side: "both_use"` or `"uses_both"` to identify shared dependencies or dual callers.
   - Call `graph_history_hotspots` with `{ "project": "<repo-root>" }` to correlate architectural hotspots with Git commit frequency and co-change peers.

## Output Checklist

- Report concrete violating edges (`from -> to`) from `graph_architecture` and whether `certain: true` eliminates any guessed links.
- Quote shortest cycle paths returned by `graph_cycle`.
- Highlight churn + fan-in overlaps from `graph_history_hotspots`.
