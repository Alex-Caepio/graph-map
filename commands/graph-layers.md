---
name: graph-layers
description: Audit architectural layers, pattern roles, module boundaries, and shortest dependency cycles for the current project.
argument-hint: "[<scope-or-node>] [--modules]"
---

Audit architectural boundaries and dependency cycles for the repository (`$ARGUMENTS` optional scope/node/target).

**Important**: Always pass `project: "<absolute-repository-root>"` to `graph_*` MCP tools.

1. Ensure graph availability via `graph_status` (`project: "<repo-root>"`, `check: true`); run `graph_build` if unbuilt.
2. Call `graph_architecture` (`project: "<repo-root>"`, optional `scope`/`pattern`/`modules`/`lod`/`target`).
3. If a specific node/folder or cycle check is requested, call `graph_cycle` (`project: "<repo-root>"`, `cycle: "<node>"`).
4. Report layer/module membership, any boundary violations (`from -> to`), and cycle return paths.
