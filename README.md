# Graph Map (`graph-map`)

Standalone interactive code-graph explorer for codebase dependencies, Git commits, pull requests, architectural layers, test reach, and AI coding run audits — packaged as a native Rust CLI, stdio JSON-RPC 2.0 MCP server (14 tools), embedded localhost WASM UI, and dual Claude Code / Codex CLI plugin.

## Capabilities

- **Codebase & Change Impact Mapping**: Explore folders, files, symbols, shortest paths, shared coupling, downstream scenarios, cycles, architectural layers (`.graph-map/layers` rules), and Rust Cargo crate graphs (`graph_crates`).
- **Git & Target Diffs**: Map `work`, `uncommitted`, `commit:<sha>`, `branch:<name>`, `pull:<number>`, stacked PR base comparisons, and co-change commit hotspots.
- **Test Reach**: Compute shortest transitive incoming dependency chains from test files to modified files.
- **Multi-Host Agent Run Audits**: Read run transcripts directly from Harness event JSON (`.graph-map/runs/*.json`), Claude Code JSONL (`~/.claude/projects/`), and Codex CLI JSONL (`~/.codex/sessions/`) to see what an agent changed vs what dependent files it actually opened or shell-read.
- **Embedded Interactive WASM UI**: Zero external web asset dependencies — compiled WASM, JS, CSS, SVG, and HTML are embedded inside `./bin/graph-map` and served from `http://127.0.0.1:7840`.

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
./bin/graph-map --project /path/to/repo status --check
./bin/graph-map --project /path/to/repo build --import /path/to/graph.json
./bin/graph-map --project /path/to/repo find Order --json
./bin/graph-map --project /path/to/repo neighbors src/Service/OrderService.php
./bin/graph-map --project /path/to/repo link src/A.php src/B.php
./bin/graph-map --project /path/to/repo path src/A.php src/C.php
./bin/graph-map --project /path/to/repo shared src/A.php src/B.php --side both_use
./bin/graph-map --project /path/to/repo cycles src/A.php
./bin/graph-map --project /path/to/repo impact work
./bin/graph-map --project /path/to/repo layers --modules
./bin/graph-map --project /path/to/repo scenario src/Controller/OrderController.php
./bin/graph-map --project /path/to/repo test-reach work
./bin/graph-map --project /path/to/repo hotspots
./bin/graph-map --project /path/to/repo runs
./bin/graph-map --project /path/to/repo explain src/A.php --to src/B.php
./bin/graph-map --project /path/to/repo serve --host 127.0.0.1 --port 7840
./bin/graph-map mcp
```

## MCP Tools (14)

- `graph_status` — Graph availability, node/edge counts, staleness, build progress
- `graph_build` — Enable, build (`GRAPH_MAP_GRAPHIFY`/`graphify`), rebuild, or import `graph.json`
- `graph_find` — Search files, folders, classes, functions, methods
- `graph_neighbors` — Step-by-step caller/callee neighborhood unfolding (`out`, `into`)
- `graph_link` — Symbol-level edge inspection between two places
- `graph_path` — Shortest directed dependency path between two places
- `graph_shared` — Shared callees (`both_use`) or shared callers (`uses_both`)
- `graph_cycle` — Shortest cycle tracing back to an origin
- `graph_change_impact` — Impact map for `work`, `uncommitted`, commits, branches, PRs, or runs
- `graph_architecture` — Layering, violations, design patterns, module boundaries
- `graph_scenario` — Forward directed execution trace from an entry point
- `graph_test_reach` — Shortest transitive incoming test paths to modified files
- `graph_history_hotspots` — Git co-change hotspots and commit history browser
- `graph_run_audit` — List or audit Claude Code, Codex, and Harness runs

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
