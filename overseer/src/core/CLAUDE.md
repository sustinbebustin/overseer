# CORE BUSINESS LOGIC

Orchestrates task operations, validation, context assembly, learning inheritance.

## FILES

| File | Lines | Purpose |
|------|-------|---------|
| `task_service.rs` | ~1500 | Task CRUD, validation, cycle detection, depth enforcement |
| `workflow_service.rs` | ~1200 | Task lifecycle: start/complete with VCS |
| `context.rs` | ~480 | Context chain assembly, learning aggregation |

## KEY ALGORITHMS

### DFS Cycle Detection (task_service.rs)
- **Parent cycles**: Linear traversal up parent chain
- **Blocker cycles**: DFS with HashSet visited, stack-based iteration
- Early termination on cycle found

### Next Ready Resolution (task_service.rs)
- `next_ready()`: DFS to find deepest unblocked leaf
- `resolve_start_target()`: Follows blockers to find startable work
- `collect_incomplete_leaves()`: Recursive leaf path collection

### Context Chain (context.rs)
- Depth 0 (Milestone): own context only
- Depth 1 (Task): own + milestone (parent)
- Depth 2 (Subtask): own + parent + milestone (grandparent)

### Learnings Bubbling (workflow_service.rs)
On completion: copy learnings to immediate parent (preserves `source_task_id`). Idempotent via unique index on `(task_id, source_task_id, content)`.

## LIFECYCLE STATE MACHINE

State COMPUTED from fields (not stored as enum):
`archived > cancelled > completed > started > pending`

| From | To | Via |
|------|----|-----|
| Pending/InProgress | Completed | `complete()` |
| Pending/InProgress | Cancelled | `cancel()` |
| Completed/Cancelled | Archived | `archive()` |
| Completed | Pending | `reopen()` (keeps started_at -> InProgress) |

- Cancelled does NOT satisfy blockers
- Cannot create children under inactive parents

## WORKFLOW SEMANTICS

### start()
1. Guard non-active states (completed/cancelled/archived)
2. Idempotent: if started_at + bookmark set, just `checkout(bookmark)`
3. `validate_start_target()` - must be next-ready in subtree
4. `create_bookmark("task/{id}")` (idempotent, BookmarkExists OK)
5. `checkout(bookmark)` - can fail with DirtyWorkingCopy
6. Record `start_commit`, set `started_at`
7. `bubble_start_to_ancestors()` - sets started_at on ancestors (no VCS state)

### complete()
1. VCS: `commit("Complete: {desc}")` - NothingToCommit OK
2. DB: mark complete, add learnings, bubble learnings to parent
3. VCS cleanup (best-effort): checkout tip/start_commit, delete bookmark
4. `bubble_up_completion()` - auto-complete parents if all siblings finished

### `effectively_blocked`
Computed post-fetch by TaskService, not stored. DB's ready filter is approximation (direct blockers only); full ancestor-aware readiness computed in Rust.

## PATTERNS

- `TaskService<'a>` / `TaskWorkflowService<'a>`: DB conn by reference
- Validation order: existence -> cycles -> depth -> mutation
- Transaction order: VCS ops first, then DB state update
- WorkflowService takes `Box<dyn VcsBackend>` (not Option)

## NON-OBVIOUS

- `context` dual-field serde trick: raw `context` (skip_serializing) + `context_chain` (serialized as "context"). Two fields, same JSON key, one suppressed.
- VCS state survives cancel: `bookmark` and `started_at` remain in DB. Start guard must check lifecycle BEFORE idempotent path.
- Bookmark naming: `task/{full_prefixed_ulid_id}`

## INVARIANTS

1. MAX_DEPTH = 2 (3 levels: 0, 1, 2)
2. Cycle detection BEFORE depth check
3. Depth always recomputed, never trusted from DB
4. VCS required for start/complete - CRUD works without VCS
5. Milestone completion cleans ALL descendant bookmarks + own
6. Blocker edges preserved on completion - readiness from blocker state
7. Archive milestone: validates ALL descendants finished first, then cascades
