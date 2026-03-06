# Architecture

System design, data model, and invariants for Overseer.

**Scope:** System map for understanding how Overseer works. Not a CLI reference ([CLI.md](CLI.md)), MCP usage guide ([MCP.md](MCP.md)), or UI component docs ([../ui/AGENTS.md](../ui/AGENTS.md)).

## Overview

Overseer is a SQLite-backed task graph manager with:
- **Rust CLI** (`os`) as source of truth for all business logic
- **Node MCP server** providing codemode interface for agents  
- **Web UI** for visual task inspection
- **VCS integration** (jj-first) for workflow operations

```
┌─────────────────────────────────────────────────────────────┐
│                    Entry Points                             │
├─────────────────┬─────────────────┬─────────────────────────┤
│   MCP Server    │    UI Server    │     Direct CLI          │
│   (Node.js)     │  (Hono/Node)    │                         │
│   execute tool  │  /api/* routes  │   os task list          │
└────────┬────────┴────────┬────────┴───────────┬─────────────┘
         │                 │                    │
         └─────────────────┼────────────────────┘
                           │ spawn os --json ...
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                      os CLI (Rust)                          │
│  - All business logic (validation, cycles, workflows)       │
│  - SQLite persistence ($CWD/.overseer/tasks.db)             │
│  - VCS backends: jj-lib (primary), gix (fallback)           │
└─────────────────────────────────────────────────────────────┘
```

**Key insight:** No long-running Rust daemon. Node pieces are thin shells that spawn the CLI.

## Why This Architecture?

| Decision | Rationale |
|----------|-----------|
| **Rust CLI core** | Testable, reusable, performant, type-safe |
| **Node MCP wrapper** | MCP SDK is JS, codemode needs V8 sandbox |
| **SQLite not JSON** | Queries, transactions, concurrent safe |
| **jj-lib not shell** | Native performance, no spawn overhead |
| **gix not git2** | Pure Rust, no C deps, actively maintained |
| **JJ-first** | Primary VCS, git as fallback |
| **ULID IDs** | Sortable, no central coordination |

## Package Structure

```
overseer/
├── overseer/        # Rust CLI (binary: os)
│   └── src/
│       ├── core/    # task_service (1471), workflow_service (1208), context (481)
│       ├── db/      # SQLite repos
│       └── vcs/     # jj.rs (754), git.rs (854)
├── mcp/             # Node MCP codemode server
├── ui/              # Hono API + Vite + React SPA
└── npm/             # Publishing: wrapper + platform binaries
```

## Domain Model

### Task Hierarchy (Tree)

Tasks form a tree with **max depth = 2** (3 levels total):

| Depth | Name | Parent |
|-------|------|--------|
| 0 | Milestone | None (root) |
| 1 | Task | Milestone |
| 2 | Subtask | Task |

**Depth is computed from parent chain, not stored.**

### Blockers (DAG)

`task_blockers(task_id, blocker_id)` defines dependencies between tasks.

**Ready:** Task is ready when not completed AND all direct blockers completed.

**Effectively blocked:** Task OR any ancestor has incomplete blockers. Subtrees inherit blocked-ness.

### Learnings

Learnings are knowledge captured during task work. They **bubble upward on completion**:

1. Learnings attached to completed task
2. **Copied to immediate parent** (preserves original `source_task_id`)
3. Siblings see learnings after their tasks complete and merge

**Idempotency:** Unique index on `(task_id, source_task_id, content)` prevents duplicates on re-bubble.

## Persistence

### Tables

| Table | Purpose |
|-------|---------|
| `tasks` | Core fields + workflow (`started_at`, `bookmark`, `start_commit`, `commit_sha`) |
| `learnings` | Content + `source_task_id` for attribution |
| `task_blockers` | Dependency edges |
| `task_metadata` | Reserved for extensibility |

**ID constraints:** CHECK constraints enforce `task_*` and `lrn_*` prefixes.

**CASCADE deletes:** Deleting a task removes descendants, learnings, and blocker edges.

### Schema Versioning

- `SCHEMA_VERSION = 3` (in `overseer/src/db/schema.rs`)
- `PRAGMA user_version` tracks version
- WAL mode enabled for concurrent access

## Core Workflows

### CRUD vs Workflow Operations

| Operation | VCS Required? |
|-----------|---------------|
| create, list, get, update, delete | No |
| block, unblock, reopen | No |
| **start, complete** | **Yes** |

Workflow ops fail with `NotARepository` if no VCS found, or `DirtyWorkingCopy` on uncommitted changes.

### Start Semantics

`start(id)` performs:

1. **Validate** task is startable (not blocked, is next-ready target)
2. **Create bookmark** (idempotent - tolerates "already exists")
3. **Checkout** bookmark
4. **Capture** git branch name as `base_ref` (git only; detached/unborn fail start)
5. **Record** `start_commit` SHA
6. **Persist** bookmark name + timestamps in DB
6. **Bubble `started_at`** to ancestors (timestamps only, no bookmarks)

**Idempotency:** If `started_at` + `bookmark` already set, just checkout.

### Complete Semantics

`complete(id, { result?, learnings? })` performs in order:

1. **VCS commit** (NothingToCommit = success)
2. **Git integration gate:** `merge --ff-only` from task bookmark into `base_ref` (fail-closed)
3. **Mark complete** in DB + attach learnings
4. **Bubble learnings** to immediate parent
5. **Delete bookmark** (best-effort; clear DB field only on success)
6. **Auto-complete ancestors** if all children done and unblocked

**Important:** Auto-completing parents is DB-only (no extra commit). Milestone completion does run commit logic.

### Milestone Completion

Completing a milestone triggers best-effort deletion of **ALL descendant bookmarks** (depth 1 and 2), not just direct children.

### Delete Cleanup

On task delete:
- Prefetch bookmarks before CASCADE removes rows
- Best-effort delete VCS bookmarks (failure doesn't block deletion)

## Key Algorithms

### Cycle Detection (DFS, not depth limit)

**Parent cycles:** Walk parent chain upward (linear).

**Blocker cycles:** DFS over blocker edges with HashSet visited set.

**Anti-pattern:** Never use depth limit as cycle detection.

### `next_ready()` - Deepest Unblocked Leaf

DFS from milestone (or across milestones by priority):
- Returns deepest incomplete + effectively unblocked leaf
- If node's children all complete, node itself is returned

### `resolve_start_target()` - Follow Blockers

When starting a blocked task:
- Follows incomplete blockers to find actually startable work
- Detects blocker cycles during traversal

### Context Chain Assembly

`TaskWithContext` assembles context/learnings by depth:

| Depth | Own | Parent | Milestone |
|-------|-----|--------|-----------|
| 0 | ✓ | - | - |
| 1 | ✓ | - | ✓ |
| 2 | ✓ | ✓ | ✓ |

## VCS Subsystem

### Detection (jj-first)

Walk up from cwd:
1. `.jj/` found → `JjBackend` (jj-lib)
2. `.git/` found → `GixBackend` (gix + git CLI for commits)
3. Neither → `VcsType::None`

### Backend Invariants

- **jj-lib is primary**, gix is fallback
- Never cache `Workspace`/`ReadonlyRepo` - reload each operation
- gix uses git CLI for `commit()` (gix staging API unstable)

### Workflow VCS Operations

| Operation | VCS Action |
|-----------|------------|
| start | `current_branch_name` (git), `create_bookmark`, `checkout`, `current_commit_id` |
| complete | `commit`, `merge_fast_forward` (git), `delete_bookmark` (best-effort) |
| delete | `delete_bookmark` (best-effort) |

## Public Surfaces

### Rust CLI (`os`)

Authoritative source. Supports `--json` for machine output.

```bash
os task list --ready          # Human output
os --json task list --ready   # JSON output (for MCP/UI)
```

### MCP Codemode Server

Single `execute` tool. VM sandbox exposes:

```javascript
{
  tasks: { create, get, list, update, start, complete, reopen, delete, block, unblock, nextReady, tree, search, progress },
  learnings: { list },
  console, setTimeout, Promise
}
```

**No `vcs` API in sandbox** - VCS is integrated into `tasks.start`/`tasks.complete`.

**Security:** 30s timeout, 50k char output limit, no fs/network/process access.

### Web UI

- **Hono API server** spawns `os --json ...`
- **React SPA** uses TanStack Query against Hono endpoints
- **Tailwind v4** for styling (OKLCH colors, monospace typography)

Dev: Vite proxies `/api/*` to Hono. Prod: Hono serves static `dist/`.

## Type Contracts

Rust JSON output is source of truth. TypeScript mirrors it.

**Files that must stay in sync:**

| Rust | TypeScript |
|------|------------|
| `overseer/src/types.rs` | `mcp/src/types.ts` |
| `overseer/src/core/context.rs` | `ui/src/types.ts` |

**Contract:** `serde(rename_all = "camelCase")` on all Rust structs.

**Caveat:** `InheritedLearnings` in `types.rs` (schema/export) differs from `context.rs` (runtime) - the runtime version includes `own`.

## Distribution

### npm Package Structure

`@dmmulroy/overseer` (main package):
- Node router `bin/os`:
  - `os mcp` → starts MCP server
  - `os ui` → starts bundled UI server
  - Otherwise → forwards to native platform binary
- optionalDependencies on platform packages

`@dmmulroy/overseer-<platform>`:
- Contains native `os` binary
- chmod postinstall for executable

**Environment variables:**
- `OVERSEER_CLI_PATH` - Override CLI binary path
- `OVERSEER_CLI_CWD` - Override working directory
- `PORT` - UI server port (default: 6969)

## Guardrails

**Anti-patterns (never do):**
- Guess VCS type - always detect via `detection.rs`
- Use depth limit for cycle detection - use DFS
- Bypass CASCADE delete invariant
- Cache jj `Workspace`/`ReadonlyRepo`
- Skip `rebase_descendants()` after `rewrite_commit()` in jj

**Invariants (always true):**
- VCS operations run before DB updates in workflow
- Bookmark created on start, deleted on complete (best-effort)
- Milestone completion cleans ALL descendant bookmarks
- Learnings bubble to immediate parent only (preserves source_task_id)

## References

- [CLI Reference](CLI.md) - Complete command documentation
- [MCP Guide](MCP.md) - Agent usage patterns
- [UI Knowledge Base](../ui/AGENTS.md) - UI patterns and conventions
