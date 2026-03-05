# DB LAYER

SQLite persistence with FK enforcement, CASCADE deletes, prefixed ID constraints.

## FILES

| File | Lines | Purpose |
|------|-------|---------|
| `schema.rs` | ~70 | DDL, connection mgmt, FK enforcement |
| `task_repo.rs` | ~500 | Task CRUD, blockers, hierarchy queries |
| `learning_repo.rs` | ~280 | Learning CRUD, task association |

## SCHEMA

```sql
tasks (id TEXT PK CHECK(id LIKE 'task_%'), parent_id FK CASCADE, ...)
learnings (id TEXT PK CHECK(id LIKE 'lrn_%'), task_id FK CASCADE, ...)
task_blockers (task_id, blocker_id -- both FK CASCADE, composite PK)
task_metadata (task_id PK FK CASCADE, data TEXT)
```

## SCHEMA VERSION: 5

Migration history: (1) initial, (2) bookmark+start_commit cols, (3) unique learning index + backfill, (4) priority 1-5 -> 0-2 collapse, (5) cancelled/archived cols.

Key indexes: `idx_tasks_parent`, `idx_learnings_unique(task_id, source_task_id, content)`, `idx_blockers_blocker`.

## PATTERNS

- `PRAGMA foreign_keys = ON` on every connection
- `PRAGMA journal_mode = WAL` for concurrent reads
- Schema versioning via `user_version` pragma
- CASCADE: delete task -> children, learnings, blockers all removed
- `row_to_task()`: SQLite row -> domain type mapping
- Dynamic query builder: SQL + `Box<dyn ToSql>` params vector
- Depth: computed at read time via iterative parent walk or recursive CTE (never stored)
- Ready filter: not completed AND all blockers completed
- `get_all_descendants()`: recursive collection for milestone cleanup
- `satisfies_blocker()` = completed only (cancelled does NOT satisfy)
- Learning bubbling: `INSERT OR IGNORE` with unique index for idempotency
