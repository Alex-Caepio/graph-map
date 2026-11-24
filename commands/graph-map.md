---
name: graph-map
description: Check code graph status, search symbols/files, inspect neighbors, or launch the interactive localhost WASM Dependency Map UI.
argument-hint: "[serve | <file-or-symbol>]"
---

Inspect the dependency graph or launch the interactive browser map for the current project.

**Important**: Always pass `project: "<absolute-repository-root>"` to `graph_*` MCP tools.

1. Check graph status with `graph_status` (`project: "<repo-root>"`, `check: true`).
2. If no graph is built yet (`state == "off"` and no `fallback_graph`), run `graph_build` with `project: "<repo-root>"` (builds the native 13-language AST + SQLite IR index automatically).
3. If `$ARGUMENTS` is provided:
   - If it looks like `"serve"` or `"ui"`, instruct or run `bin/graph-map --project "<repo-root>" serve` to launch the interactive localhost Canvas2D/WASM map at `http://127.0.0.1:7840`.
   - If it mentions a file path or symbol, run `graph_find` or `graph_neighbors` (supporting `depth`, `lod`, `min_weight`).
4. Summarize graph node/edge counts, backend/staleness, or matching results cleanly.
