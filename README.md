# Graph Map (`graph-map`)

Standalone interactive code-graph explorer for codebase dependencies, Git commits, `A..B` commit ranges, pull requests, architectural layers, linked project memory, test reach, and AI coding run audits — packaged as a native Rust CLI, stdio JSON-RPC 2.0 MCP server (16 tools), embedded localhost Canvas2D WASM UI, and dual Claude Code / Codex CLI plugin.

## Capabilities

- **Native 13-Language AST + Incremental SQLite IR**: Indexes Rust, PHP, TypeScript/TSX, JavaScript, Python, Go, Java, C#, C, C++, Ruby, HTML, and CSS natively via tree-sitter into SQLite IR without requiring Python or `graphify` (optional external `graphify` or `graph.json` import remain supported).
- **Codebase & Change Impact Mapping**: Explore progressive `overview` / `folder` / `file` / `symbol` LOD slices, `min_weight` edge filtering, shortest/near-shortest paths, shared coupling, downstream scenarios, cycles, architectural layers (`.graph-map/layers` / `.harness-architecture.json`), and Rust Cargo crate graphs (`graph_crates`).
- **Git Ranges, PRs & Memory Links**: Map `work`, `uncommitted`, `commit:<sha>`, `ref:<name>`, `pr:<number>`, `range:<from>..<to>`, stacked PR base comparisons, per-file blast reports (`file-impact`), co-change hotspots, and Project Brain / Memory Bank citations (`memory-links`).
- **Test Reach & Multi-Host Agent Run Audits**: Compute shortest transitive incoming test chains to modified files, and audit Harness (`.graph-map/runs/*.json`), Claude Code (`~/.claude/projects/`), and Codex (`~/.codex/sessions/`) transcripts against what an agent edited vs opened.
- **Embedded Interactive Canvas2D WASM UI**: Zero external web asset dependencies — 5 analytical lenses, commit DAG browser, display/LOD controls, SSE `/api/events/stream` updates, and dense Canvas2D rendering compiled to WASM and embedded inside `./bin/graph-map` (`http://127.0.0.1:7840`).

## Installation & Plugin Setup

### Install via GitHub

**Claude Code:**

```bash
claude plugin marketplace add Alex-Caepio/ai-infra-plugins
claude plugin install graph-map@ai-infra-plugins
```

**Codex CLI:**

```bash
codex plugin marketplace add https://github.com/Alex-Caepio/ai-infra-plugins.git
codex plugin add graph-map@ai-infra-plugins
```

Source repositories:
- Standalone plugin: [Alex-Caepio/graph-map](https://github.com/Alex-Caepio/graph-map)
- Marketplace catalog: [Alex-Caepio/ai-infra-plugins](https://github.com/Alex-Caepio/ai-infra-plugins)

### Plugin Manifests & Local Binary

The prebuilt Linux x86-64 binary lives at `./bin/graph-map`.

- **Claude Code**: Uses `.claude-plugin/plugin.json`, `.mcp.json` (`${CLAUDE_PLUGIN_ROOT}/bin/graph-map mcp`), `skills/`, and `commands/`.
- **Codex CLI**: Uses `.codex-plugin/plugin.json`, `.codex-mcp.json` (`"command": "./bin/graph-map"`, `"cwd": "."`), and `skills/`.

> **Note for MCP callers**: Because Codex starts MCP servers with `cwd` inside the cached plugin directory, always pass `project: "<repository-root>"` to `graph_*` MCP tools (or set `project` explicitly in arguments). Calling MCP tools from inside a plugin directory without `project` is rejected by design so it never inspects the plugin cache or an ancestor directory accidentally.

## CLI Usage

```bash
./bin/graph-map --project /path/to/repo build
./bin/graph-map --project /path/to/repo status --check
./bin/graph-map --project /path/to/repo map --lod folder --min-weight 2
./bin/graph-map --project /path/to/repo find Order --json
./bin/graph-map --project /path/to/repo neighbors src/Service/OrderService.php --depth 2
./bin/graph-map --project /path/to/repo link src/A.php src/B.php
./bin/graph-map --project /path/to/repo path src/A.php src/C.php --longer 1
./bin/graph-map --project /path/to/repo shared src/A.php src/B.php --side both_use
./bin/graph-map --project /path/to/repo cycles src/A.php
./bin/graph-map --project /path/to/repo impact --from main --to HEAD --lod file
./bin/graph-map --project /path/to/repo changes --from main --to HEAD
./bin/graph-map --project /path/to/repo file-impact src/Entity/Order.php work
./bin/graph-map --project /path/to/repo memory-links src/Entity/Order.php
./bin/graph-map --project /path/to/repo layers --modules
./bin/graph-map --project /path/to/repo scenario src/Controller/OrderController.php
./bin/graph-map --project /path/to/repo test-reach work
./bin/graph-map --project /path/to/repo hotspots
./bin/graph-map --project /path/to/repo runs
./bin/graph-map --project /path/to/repo explain src/A.php --to src/B.php
./bin/graph-map --project /path/to/repo serve --host 127.0.0.1 --port 7840
./bin/graph-map mcp
```

## MCP Tools (16)

- `graph_status` — Graph availability, backend, node/edge counts, staleness, build progress
- `graph_build` — Build native 13-language AST/SQLite IR (or optional `graphify` / `import` `graph.json`)
- `graph_find` — Search files, folders, classes, functions, methods
- `graph_neighbors` — Caller/callee unfolding (`depth`, `out`, `into`, `lod`, `min_weight`)
- `graph_link` — Symbol-level edge inspection between two places
- `graph_path` — Shortest and near-shortest (`longer`) directed paths between two places
- `graph_shared` — Shared callees (`both_use`) or shared callers (`uses_both`)
- `graph_cycle` — Shortest cycle tracing back to an origin
- `graph_change_impact` — Impact map for `work`, `uncommitted`, commits, `from`/`to` ranges, PRs, or runs
- `graph_architecture` — Layering, violations, design patterns, module boundaries, root nodes
- `graph_scenario` — Forward directed execution trace from an entry point
- `graph_test_reach` — Shortest transitive incoming test paths to modified files
- `graph_history_hotspots` — Git co-change hotspots and commit history browser
- `graph_run_audit` — List or audit Claude Code, Codex, and Harness runs
- `graph_file_impact` — Structured per-file blast radius, symbol diffs, and reaching tests
- `graph_memory_links` — Lookup Project Brain and Memory Bank citations for single or batch files

## State & Graph Resolution

- Standalone state lives in `$GRAPH_MAP_STATE` (or `$XDG_STATE_HOME/graph-map` / `~/.local/state/graph-map`).
- For read/query commands, if no managed build exists yet, `graph-map` automatically inspects `<repo>/graphify-out/graph.json` and `<repo>/.graph-map/graph.json`, or `--graph <path>`.

## Building from Source

```bash
wasm-bindgen --target web --no-typescript --out-name graph_map_ui --out-dir crates/graph-map-cli/web/assets ...
cargo build --release --locked --bin graph-map
```

## License

Apache-2.0 — see [LICENSE](LICENSE).
