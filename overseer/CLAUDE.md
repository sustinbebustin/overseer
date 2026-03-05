# OVERSEER CLI (Rust)

Binary: `os`. All business logic lives here - host just spawns and parses JSON.

## STRUCTURE

```
src/
├── main.rs              # clap CLI, JSON/human output dispatch (484 lines)
├── lib.rs               # Re-exports for integration tests
├── commands/            # Subcommand handlers (task, learning, vcs, data)
├── core/
│   ├── task_service.rs  # Validation, cycles, hierarchy (~1500 lines)
│   ├── workflow_service.rs # Start/complete with VCS (~1200 lines)
│   └── context.rs       # Context chain assembly (~480 lines)
├── db/
│   ├── schema.rs        # DDL, migrations, FK enforcement
│   ├── task_repo.rs     # Task CRUD + blockers (~500 lines)
│   └── learning_repo.rs # Learning CRUD (~280 lines)
├── vcs/
│   ├── detection.rs     # Detect .jj/ vs .git/
│   ├── backend.rs       # VcsBackend trait
│   ├── jj.rs            # jj-lib native (~650 lines)
│   └── git.rs           # gix + git CLI fallback (~730 lines)
├── error.rs             # OsError enum (thiserror)
├── types.rs             # Task, CreateTaskInput, filters
└── id.rs                # TaskId, LearningId (prefixed ULIDs)
```

## DATA FLOW

```
main.rs (clap parse)
  -> commands/*.rs (dispatch)
    -> core/task_service.rs (validation, cycles)
      -> db/task_repo.rs (SQL)
    -> core/workflow_service.rs (VCS integration)
      -> vcs/*.rs (jj-lib or gix)
  -> print_human() or JSON output
```

## WHERE TO LOOK

| Task | File | Notes |
|------|------|-------|
| Add CLI subcommand | `commands/{name}.rs` | Wire in `commands/mod.rs` + `main.rs` |
| Task validation | `core/task_service.rs` | Depth, cycles, blockers |
| Task lifecycle | `core/workflow_service.rs` | Start/complete with VCS |
| SQL queries | `db/task_repo.rs` | All raw SQL here |
| Schema changes | `db/schema.rs` | Bump `SCHEMA_VERSION` |
| VCS detection | `vcs/detection.rs` | Returns (VcsType, Option<PathBuf>) |
| Error variants | `error.rs` | Add to `OsError` enum |
| New ID type | `id.rs` | Follow TaskId pattern |

## CONVENTIONS

- `Result<T>` = `Result<T, OsError>` (aliased in error.rs)
- Prefixed IDs: `task_01ARZ...`, `lrn_01ARZ...` stored with prefix
- `serde(rename_all = "camelCase")` for JSON output
- `pollster::block_on` for jj-lib async at boundaries
- Clone commands before handle() (clap ownership)
- VcsBackend trait for all VCS ops, not concrete types

## ANTI-PATTERNS

- Never bypass `TaskService` for task mutations
- Never use depth limit for cycle detection - DFS only
- Never hardcode VCS type - always detect via `detection.rs`
- Never store IDs without prefix - CHECK constraints enforce
- Never skip `PRAGMA foreign_keys = ON`

## COMMANDS

```bash
(cd overseer && cargo build --release)
(cd overseer && cargo test)
(cd overseer && cargo test -- --nocapture)
./overseer/target/release/os --help
./overseer/target/release/os --json task list
```

## TESTS

| Location | Type |
|----------|------|
| `tests/*.rs` | Integration (3 files) |
| `src/**/*.rs` | Unit (inline #[test]) |
| `testutil.rs` | Helpers: JjTestRepo, GitTestRepo |

## KEY DEPENDENCIES

- `jj-lib =0.37` - Pinned exactly (API breaks between minors)
- `gix 0.77` - Pure Rust git
- `rusqlite` - SQLite with bundled feature
- `clap 4.5` - CLI parsing
- `thiserror 2.0` - Error handling
- `chrono` - Timestamps
- `ulid` - ID generation
