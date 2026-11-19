---
name: graph-map
description: Check code graph status, search symbols/files, inspect neighbors, or launch the interactive localhost WASM Dependency Map UI.
---

Inspect the dependency graph or launch the interactive browser map for the current project.

**Important**: Always pass `project: "<absolute-repository-root>"` to `graph_*` MCP tools.

1. Check graph status with `graph_status` (`project: "<repo-root>"`, `check: true`).
2. If `$ARGUMENTS` is provided:
   - If it looks like `"serve"` or `"ui"`, instruct or run `bin/graph-map --project "<repo-root>" serve` to launch the interactive localhost WASM map at `http://127.0.0.1:7840`.
   - If it mentions a file path or symbol, run `graph_find` or `graph_neighbors`.
3. If no graph is built yet (`state == "off"` and no `fallback_graph`), run `graph_build` with `project: "<repo-root>"` (or `--import <graph.json>`).
4. Summarize graph node/edge counts, staleness, or matching results clearly.
