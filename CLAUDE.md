# OVERSEER

Codemode MCP server for AI agent task management. Rust CLI (`os`) + Node host + React viewer. SQLite-backed, native VCS (jj-lib + gix).

## ARCHITECTURE

```
Agent (Claude, etc.)
    |
    v  (MCP stdio / codemode)
+-----------------------------------+
|  host/ (Node.js)                  |
|  - Single "execute" tool          |
|  - VM sandbox: tasks/learnings    |
|  - Spawns `os` CLI, parses JSON   |
|  - Also serves UI (Hono HTTP)     |
+-----------------------------------+
    |
    v  (spawn `os --json ...`)
+-----------------------------------+
|  overseer/ (Rust CLI: `os`)       |
|  - All business logic             |
|  - SQLite storage                 |
|  - Native VCS: jj-lib + gix      |
+-----------------------------------+
```

## STRUCTURE

```
overseer/
├── overseer/          # Rust CLI (binary: os)
│   └── src/
│       ├── main.rs    # clap CLI, JSON/human output
│       ├── commands/  # Subcommand handlers
│       ├── core/      # TaskService, WorkflowService, context
│       ├── db/        # SQLite repos
│       └── vcs/       # jj-lib + gix backends
│
├── host/              # Node host (MCP + UI server)
│   └── src/
│       ├── index.ts   # Entry: `overseer-host mcp|ui`
│       ├── mcp.ts     # MCP server (execute tool)
│       ├── executor.ts # VM sandbox, CLI bridge
│       ├── cli.ts     # CLI spawn wrapper
│       ├── ui.ts      # Hono HTTP + static serving
│       └── api/       # tasks/learnings API bindings
│
├── ui/                # Task Viewer (Hono API + Vite + React)
│   └── src/
│       ├── api/       # Hono API server (CLI bridge)
│       ├── client/    # React SPA (components, queries, state)
│       └── types.ts   # Shared types
│
├── npm/               # npm publishing (platform-specific binaries)
├── skills/            # Agent skills (skills.sh compatible)
├── generated/         # Auto-generated TS types (regenerate: ./scripts/generate-types.sh)
└── docs/              # Architecture, CLI ref, MCP ref
```

## WHERE TO LOOK

| Task | Location | Notes |
|------|----------|-------|
| Add CLI command | `overseer/src/commands/` | Wire in mod.rs + main.rs |
| Add MCP API | `host/src/api/` | Export in api/index.ts |
| Task CRUD | `overseer/src/db/task_repo.rs` | SQL layer |
| Task business logic | `overseer/src/core/task_service.rs` | Validation, hierarchy |
| Task lifecycle | `overseer/src/core/workflow_service.rs` | VCS integration |
| VCS operations | `overseer/src/vcs/` | jj.rs (primary), git.rs (fallback) |
| Error types | `overseer/src/error.rs` | OsError enum |
| Types/IDs | `overseer/src/types.rs`, `overseer/src/id.rs` | Domain types, ULID |
| UI API routes | `ui/src/api/routes/` | Hono route handlers |
| UI components | `ui/src/client/components/` | React components |
| UI queries | `ui/src/client/lib/queries.ts` | TanStack Query hooks |
| UI theme | `ui/src/client/styles/global.css` | Tailwind v4, OKLCH tokens |

## KEY DECISIONS

| Decision | Choice | Rationale |
|----------|--------|-----------|
| CLI binary | `os` | Short, memorable |
| Storage | SQLite (WAL) | Concurrent reads, rich queries |
| VCS primary | jj-lib | Native perf, no subprocess |
| VCS fallback | gix | Pure Rust, no C deps |
| IDs | ULID with prefix | Sortable, coordination-free, `task_`/`lrn_` |
| Task hierarchy | 3 levels max | Milestone(0) -> Task(1) -> Subtask(2) |
| MCP pattern | Codemode | Agents write JS, VM executes, results return |
| Host package | Unified `host/` | Single entry for MCP (stdio) and UI (HTTP) |

## TYPE SYNC (Rust <-> TS)

Types must stay in sync across packages:
- `TaskId`: Newtype (Rust) / Branded type (TS), `task_` prefix + 26-char ULID
- `LearningId`: Newtype / Branded, `lrn_` prefix
- Rust uses `serde(rename_all = "camelCase")` -> JSON matches TS interfaces

**When changing constrained types (e.g., Priority range):**
1. Rust: `types.rs`, validation in `task_service.rs`, CLI args in `commands/task.rs`
2. TypeScript: `host/src/types.ts`, `ui/src/types.ts`
3. Decoders: `host/src/decoder.ts`, `ui/src/decoder.ts`
4. API interfaces: `host/src/api/tasks.ts`, `host/src/ui.ts`
5. UI input constraints: min/max on number inputs

## CONVENTIONS

- **Result everywhere**: All fallible Rust ops return `Result<T, OsError>`
- **TaggedError (TS)**: Errors use `_tag` discriminator
- **No `any`**: Strict TypeScript
- **No `!`**: Non-null assertions forbidden
- **Minimize `as Type`**: Use decoders at boundaries
- **jj-first**: ALWAYS detect `.jj/` before `.git/`
- **Prefixed IDs**: `task_*`, `lrn_*` with CHECK constraints in DB

## ANTI-PATTERNS

- Never guess VCS type - detect via `vcs/detection.rs`
- Never skip cycle detection - DFS in `task_service.rs`
- Never bypass CASCADE delete invariant
- Never use depth limit for cycle detection (use DFS)
- **Falsy-0 bug**: `if (value)` fails for valid 0 - use `value !== undefined`

## LIFECYCLE STATE MACHINE

State is COMPUTED from DB fields, not stored as enum:

```
archived=true           -> Archived
cancelled=true          -> Cancelled
completed=true          -> Completed
started_at IS NOT NULL  -> InProgress
(none of above)         -> Pending
```

- Cancelled does NOT satisfy blockers (blocks dependents)
- `reopen` clears completed but NOT started_at (stays InProgress)
- Archive cascades for milestones (all descendants must be finished first)

## DESIGN INVARIANTS

1. Cycle detection via DFS (not depth limit)
2. CASCADE delete: tasks -> children + learnings + blockers
3. CLI spawn timeout: 30s in VM executor
4. Timestamps: ISO 8601 / RFC 3339
5. "Milestone" = depth-0 task (no parent)
6. Learnings bubble to immediate parent only (preserves source_task_id, idempotent via unique index)
7. VCS required for workflow ops (start/complete) - CRUD works without VCS
8. VCS bookmark lifecycle: create on start, delete on complete (unified for jj & git)
9. Milestone completion cleans ALL descendant bookmarks
10. Blocker edges preserved on completion - readiness computed from blocker state
11. Depth computed at read time (never stored) - recursive CTE or Rust walk
12. `effectively_blocked` computed post-fetch by TaskService, not stored
13. `os mcp` and `os ui` subcommands spawn `host/` as subprocess

## COMMANDS

```bash
# Rust CLI
(cd overseer && cargo build --release)
(cd overseer && cargo test)

# Host (MCP + UI server)
(cd host && npm run build)
(cd host && npm run dev)

# UI
(cd ui && npm run dev)        # Hono API + Vite HMR
(cd ui && npm run build)
(cd ui && npm run typecheck)
```

## DOCS

| Document | Purpose |
|----------|---------|
| `docs/ARCHITECTURE.md` | System design |
| `docs/CLI.md` | CLI command reference |
| `docs/MCP.md` | MCP tool/API reference |
| `ui/CLAUDE.md` | UI package knowledge base |
| `overseer/CLAUDE.md` | Rust CLI knowledge base |
| `host/CLAUDE.md` | Host package knowledge base |
