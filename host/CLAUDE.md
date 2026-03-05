# HOST PACKAGE

Unified Node.js entry for MCP server (stdio) and UI server (HTTP). Spawns `os` CLI for all operations.

## STRUCTURE

```
src/
├── index.ts       # Entry: `overseer-host mcp|ui`, arg parsing
├── mcp.ts         # MCP server: single "execute" tool registration
├── executor.ts    # VM sandbox: runs agent JS with tasks/learnings APIs
├── cli.ts         # CLI bridge: spawn `os --json`, parse JSON output
├── ui.ts          # Hono HTTP server + static file serving
├── decoder.ts     # Runtime type decoders for CLI JSON output
├── types.ts       # TypeScript types (mirrors Rust types.rs)
└── api/
    ├── index.ts   # API exports
    ├── tasks.ts   # Task operations (list, get, create, start, complete...)
    └── learnings.ts # Learning operations
```

## CODEMODE PATTERN

Agents write JS code -> `execute` tool runs it in VM sandbox -> only results return.

- `mcp.ts`: Registers single `execute` tool with full API type docs in description
- `executor.ts`: Creates VM context with `tasks` and `learnings` globals
- `cli.ts`: Bridge layer - builds `os --json` commands, spawns, parses output
- 30s execution timeout, 50k char output truncation

## KEY FILES

| Task | File |
|------|------|
| Change MCP tool description | `mcp.ts` (TOOL_DESCRIPTION) |
| Add API operation | `api/tasks.ts` or `api/learnings.ts` |
| Modify CLI spawn behavior | `cli.ts` |
| Change VM sandbox globals | `executor.ts` |
| Add runtime type validation | `decoder.ts` |
| Start UI HTTP server | `ui.ts` |

## CLI BRIDGE (cli.ts)

- `configureCli()`: Set path, cwd, dbPath at startup
- `runCli()`: Spawn `os --json --db <path>` with args
- `--db` passed on EVERY call - pins SQLite file regardless of cwd (monorepo support)
- Parses stdout as JSON, stderr for errors
- `CliError` / `CliTimeoutError` tagged errors
- For workflow ops, `repoPath` resolves against base cwd (monorepo VCS root)

## MODES

| Mode | Transport | Entry |
|------|-----------|-------|
| `mcp` | stdio | `startMcpServer()` - MCP SDK + StdioServerTransport |
| `ui` | HTTP | `startUiServer()` - Hono on configurable port |

## CONVENTIONS

- TaggedError pattern: errors use `_tag` discriminator
- No `any` - strict TypeScript
- Decoders at CLI boundary (never trust raw JSON shape)
- `better-result` for Result-type error handling
- Branded types: `TaskId = string & { [TaskIdBrand]: never }`, validated via `isTaskId()`

## VM SANDBOX SECURITY

- No `fs`, `process`, `fetch`, `require` exposed
- Max 100 simultaneous timers, all cleaned on exit
- Agent code wrapped in async IIFE (supports `return` + `await`)
- Spawned by `os mcp` CLI subcommand with `--cli-path`, `--cwd`, `--db-path` args
