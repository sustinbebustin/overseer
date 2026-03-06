# Feature: Git-safe task completion with mandatory fast-forward integration

**Status:** Ready for task breakdown  
**Type:** Feature plan (bugfix + safety hardening)  
**Effort:** L  
**Date:** 2026-03-05

## Problem Statement
**Who:** agents and users running Overseer `start -> implement -> complete` in git repos.  
**What:** current git completion can orphan commits by detaching HEAD then force-deleting task branch.  
**Why it matters:** orphaned commits are eventually garbage-collected; silent data loss.  
**Evidence:**
- incident: 9 security/correctness commits recovered only from raw SHAs in logs
- current flow in `overseer/src/core/workflow_service.rs`: commit, checkout commit id, delete bookmark/branch best-effort
- force-delete escalation in `overseer/src/vcs/git.rs` (`git branch -d` -> `git branch -D`)

## Proposed Solution
Make git completion enforce integration before DB completion.

1. Capture `base_ref` at `start()` (branch agent started from).
2. On git `complete()`, require fast-forward merge of `task/<id>` into `base_ref` **before** marking task completed.
3. Remove force-delete behavior (`-D`) globally.
4. If ff merge is not possible, return a typed error, keep task in-progress, preserve task branch.

This keeps the workflow unchanged for agents (`start -> implement -> complete`) while making data loss structurally impossible in normal paths.

## Scope & Deliverables
| Deliverable | Effort | Depends On |
|-------------|--------|------------|
| D1. Add `base_ref` storage + schema migration (`v5 -> v6`) | S | - |
| D2. Add Rust/TS type sync for `Task.baseRef` | S | D1 |
| D3. Extend `VcsBackend` for branch detection + ff integration (explicit impls, no default no-op) | M | D1 |
| D4. Implement git backend: `current_branch_name`, `merge_fast_forward`, safe delete only (`-d`, no `-D`) | M | D3 |
| D5. Update workflow `start()` and `complete()` (task + milestone) to enforce merge-before-complete semantics | L | D1, D3, D4 |
| D6. Add legacy repair path for pre-v6 started tasks (idempotent `start()` backfills `base_ref` when safe) | M | D5 |
| D7. Add unit + git integration tests for success/failure/legacy cases | L | D5, D6 |
| D8. Remove `/overseer-complete` command/docs and add recovery runbook | S | D5 |

## Non-Goals (Explicit Exclusions)
- No auto-rebase/cherry-pick/squash during `tasks.complete()`.
- No automatic creation of new blocker tasks on ff failure.
- No change to jj completion semantics beyond trait conformance.
- No change to task hierarchy, blocker graph, learnings bubbling rules.
- No export/import schema expansion for workflow-only metadata (`base_ref`).

## Data Model

### SQLite
**File:** `overseer/src/db/schema.rs`
- bump `SCHEMA_VERSION` to `6`
- add `base_ref TEXT` to `tasks` table in fresh schema
- add migration block:
  - `ALTER TABLE tasks ADD COLUMN base_ref TEXT`
  - `PRAGMA user_version = 6`

### Rust Task Type
**File:** `overseer/src/types.rs`
- add `base_ref: Option<String>` near `start_commit`
- mark `#[serde(skip_serializing_if = "Option::is_none")]`
- serialized JSON key becomes `baseRef` via `rename_all = "camelCase"`

### Task Repository
**File:** `overseer/src/db/task_repo.rs`
- `row_to_task()`: map `base_ref`
- add `set_base_ref(conn, id, base_ref)` helper
- update depth-filter CTE (`list_tasks`) to include `base_ref` in:
  - anchor SELECT
  - recursive SELECT
  - outer SELECT

### Type Sync (TS)
Update all Task surfaces with optional `baseRef?: string`:
- `generated/types.ts`
- `host/src/types.ts`
- `ui/src/types.ts`
- `host/src/decoder.ts`
- `ui/src/decoder.ts`
- `host/src/mcp.ts` (TOOL_DESCRIPTION interface docs)
- `scripts/generate-types.sh` template (prevent regen drift)

## API / Interface Contract

### No signature changes
- `tasks.start(id)` and `tasks.complete(id, options?)` signatures unchanged.

### Behavioral contract changes (git)
- `tasks.start(id)` now requires attached branch + committed HEAD to record `baseRef`.
- `tasks.complete(id)` now requires ff integration into `baseRef` before DB completion.
- ff failure returns error; task remains incomplete/in-progress; branch preserved.

### New error surfaces
Add explicit errors for deterministic automation:
- `CannotStartDetachedHead`
- `CannotStartUnbornRepository`
- `TaskIntegrationRequired { task_id, source_ref, base_ref }` (ff rejected)
- `MissingBaseRef { task_id }` (legacy started task missing base ref)

Map from VCS layer (`VcsError`) to `OsError` as needed.

## Detailed Workflow Semantics

### 1) `start(id)`
**File:** `overseer/src/core/workflow_service.rs`

Before creating task bookmark:
1. get current branch via `vcs.current_branch_name()`
2. for git:
   - detached HEAD -> fail `CannotStartDetachedHead`
   - no HEAD commit/unborn repo -> fail `CannotStartUnbornRepository`
3. create + checkout `task/<id>` as today
4. persist `bookmark`, `start_commit`, and `base_ref`

Idempotent-start upgrade path (legacy repair):
- if task already started (`started_at + bookmark`) and `base_ref` missing:
  - if current branch exists and is not the task bookmark branch, store it as `base_ref`
  - then continue normal idempotent checkout

### 2) `complete(id)` for non-milestones (git)
**File:** `overseer/src/core/workflow_service.rs`

New order:
1. lifecycle guards + idempotency checks
2. commit on task branch (`NothingToCommit` is success)
3. integration gate:
   - require `bookmark` + `base_ref`
   - call `vcs.merge_fast_forward(bookmark, base_ref)`
   - if ff rejected: return `TaskIntegrationRequired`; no DB completion; keep task branch
4. after successful ff: mark task complete in DB + learnings
5. best-effort cleanup:
   - delete task branch/bookmark with safe delete only
   - clear DB bookmark only on successful delete
6. bubble-up completion as today

If `bookmark` exists but `base_ref` missing (legacy row):
- return `MissingBaseRef`
- no DB completion, no branch deletion
- agent can run idempotent `start(id)` from intended base branch to backfill, then retry `complete`

### 3) `complete_milestone(id)`
**File:** `overseer/src/core/workflow_service.rs`

- If milestone itself has bookmark + base_ref, apply same ff gate before DB completion.
- Descendant cleanup remains best-effort, but uses safe delete only (`-d`).
- No force-delete path anywhere.

### 4) jj behavior
- `current_branch_name()` returns `Ok(None)`.
- `merge_fast_forward()` explicit impl returns success path used by jj semantics.
- Existing jj lifecycle behavior remains unchanged.

## VCS Backend Changes

### Trait
**File:** `overseer/src/vcs/backend.rs`
- add required methods (no default impl):
  - `current_branch_name() -> VcsResult<Option<String>>`
  - `merge_fast_forward(source: &str, target: &str) -> VcsResult<bool>`

### Git backend
**File:** `overseer/src/vcs/git.rs`

`current_branch_name()`:
- `git symbolic-ref --quiet --short HEAD`
- map detached state to explicit error
- verify `HEAD` exists (`git rev-parse --verify HEAD`) to reject unborn repos

`merge_fast_forward(source, target)`:
1. checkout `target`
2. `git merge --ff-only source`
3. success => `Ok(true)`
4. non-ff => checkout `source`, return `Ok(false)`
5. other errors => checkout `source` best-effort, return `Err(...)`

`delete_bookmark()`:
- remove `-D` escalation
- keep safe delete only (`git branch -d`)
- return error if not fully merged

## /overseer-complete Removal

### Why
After this change, normal integration is automatic in `tasks.complete()`. `/overseer-complete` is redundant and encourages a parallel workflow we no longer want.

### Actions
- Delete `.agents/commands/overseer-complete.md`.
- Update docs that mention completion behavior:
  - `.agents/skills/overseer/references/workflow.md`
  - `README.md`
  - `docs/ARCHITECTURE.md`
  - `docs/CLI.md`
- Ensure no docs route users to `/overseer-complete`.

## Recovery Runbook (Required)
Create `.docs/runbooks/git-task-integration-recovery.md` covering:

1. **ff failure recovery (normal, non-destructive)**
   - inspect divergence
   - rebase task branch onto base branch
   - retry `tasks.complete()`

2. **legacy missing `base_ref` recovery**
   - checkout intended base branch
   - run idempotent `tasks.start(id)` to backfill `base_ref`
   - retry `tasks.complete()`

3. **already-orphaned historical commits recovery**
   - detect dangling commits (`git fsck`)
   - recreate refs (`git branch recover/... <sha>`)
   - integrate recovered commits

Runbook must be executable by an agent without user-only steps.

## Acceptance Criteria
- [ ] No completion path in git can delete unmerged task branch via `-D`.
- [ ] Completing a git task with successful ff merge makes task commit reachable from `baseRef` branch tip.
- [ ] If ff merge fails, `tasks.complete()` returns explicit integration error and does not mark task completed.
- [ ] On ff failure, task branch remains intact and checked out on source branch (or recoverably accessible).
- [ ] `tasks.start()` fails clearly on detached HEAD and unborn git repo.
- [ ] Legacy started task with missing `base_ref` can be repaired via idempotent `tasks.start()` and then completed.
- [ ] jj workflow behavior remains unchanged.
- [ ] MCP/host/ui decoders accept and preserve optional `baseRef`.
- [ ] `/overseer-complete` command file is removed from repository.
- [ ] No user-facing docs reference `/overseer-complete` as an available workflow.
- [ ] Recovery runbook exists and is tested manually once.

## Test Strategy
| Layer | What | How |
|-------|------|-----|
| Unit (workflow) | start stores `base_ref`; idempotent start backfills legacy; complete ff success/failure/missing base_ref | Extend tests in `overseer/src/core/workflow_service.rs` with configurable mock backend |
| Unit (git backend) | branch detection, ff merge outcomes, safe delete behavior | add focused tests in `overseer/src/vcs/git.rs` |
| Integration (git) | end-to-end start/complete success, ff rejection preserves branch, detached/unborn start errors | add tests under `overseer/tests/` using `GitTestRepo` |
| Integration (jj) | regression guard for unchanged behavior | existing + targeted jj workflow test |
| Type boundary | decoder + generated type sync for `baseRef` | host/ui decode tests + regenerate types check |

## Risks & Mitigations
| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| DB write fails after successful merge | Low | Medium | keep completion retry idempotent; branch deletion happens after DB write |
| Legacy started tasks fail completion until repaired | Medium | Medium | implement idempotent-start backfill path + runbook |
| False-positive ff failure parsing from git stderr | Low | Medium | pattern-match known non-ff messages; default unknown to hard error |
| Parent auto-complete remains DB-only for non-milestones | Medium | Low | keep out-of-scope; safe delete removal eliminates data-loss path |
| Docs drift across host/ui/skill refs | Medium | Low | include docs updates in same change set and CI check where possible |
| Removing command drops a familiar manual path | Medium | Low | recovery runbook gives explicit manual git procedures |

## Trade-offs Made
| Chose | Over | Because |
|-------|------|---------|
| Fail completion on ff failure | complete+follow-up task | stronger invariant (`completed => integrated`), simpler automation |
| Explicit trait methods (no default no-op) | default impl stubs | compile-time safety across backends |
| Safe delete only (`-d`) | force delete fallback (`-D`) | eliminate orphaned-commit risk |
| Legacy repair via idempotent `start()` | auto-guess base branch at complete | avoids unsafe branch guessing |

## Implementation Notes (File Map)
- `overseer/src/db/schema.rs`
- `overseer/src/db/task_repo.rs`
- `overseer/src/types.rs`
- `overseer/src/vcs/backend.rs`
- `overseer/src/vcs/git.rs`
- `overseer/src/vcs/jj.rs`
- `overseer/src/core/workflow_service.rs`
- `overseer/src/error.rs`
- `host/src/types.ts`
- `host/src/decoder.ts`
- `host/src/mcp.ts`
- `ui/src/types.ts`
- `ui/src/decoder.ts`
- `generated/types.ts`
- `scripts/generate-types.sh`
- `.agents/commands/overseer-complete.md` (delete)
- `.agents/skills/overseer/references/workflow.md`
- `README.md`
- `docs/ARCHITECTURE.md`
- `docs/CLI.md`
- `.docs/runbooks/git-task-integration-recovery.md` (new)

## Success Metrics
- Zero new reports of orphaned task-completion commits in git mode.
- Completion failure messages become deterministic/actionable for agents.
- New tests reproduce old failure mode pre-fix and pass post-fix.
- Standard workflow docs no longer direct users to `/overseer-complete` as default.

## Unresolved Questions
- [ ] None
