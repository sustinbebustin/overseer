## Feature: Preserve tip commit across sequential task workflow

**Status:** Ready for task breakdown  
**Type:** Feature plan (bugfix)  
**Effort:** M  
**Date:** 2026-02-24

### Problem Statement
**Who:** agents/users running sequential `start -> complete -> start` workflow in a repo (git or jj).  
**What:** completion cleanup checks out `start_commit` before deleting task branch/bookmark, rewinding working copy to pre-task baseline; next task starts from that old base.  
**Why it matters:** sequential tasks can fork from stale ancestor and omit prior completed task commit from active line, violating expected stacking progression.  
**Evidence:** `overseer/src/core/workflow_service.rs` prefers `start_commit` over `current_commit_id` for checkout target in both `complete_with_learnings` and `complete_milestone_with_learnings`; reproduced in temp git repo where second task `startCommit` equals original base after first task complete.

### Proposed Solution
Adjust checkout target selection used for post-complete branch/bookmark cleanup to prefer the **current VCS HEAD** (post-commit tip) first, with existing `start_commit`-based fallback preserved for resilience.

For normal task complete:
- current: `start_commit -> current_head`
- target: `current_head -> start_commit`

For milestone complete cleanup:
- current: `milestone_start -> descendant_start -> current_head`
- target: `current_head -> milestone_start -> descendant_start`

Rationale:
- keeps detachment semantics required for git branch deletion (still checks out a commit before delete),
- preserves latest completed work as baseline for next `start()`,
- retains fallback behavior if `current_commit_id` fails.

No API/schema changes. No change to best-effort cleanup policy or bookmark clearing rules.

### Scope & Deliverables
| Deliverable | Effort | Depends On |
|-------------|--------|------------|
| D1. Update checkout target selection order in workflow service (single task + milestone paths) | S | - |
| D2. Add unit regression tests in `workflow_service.rs` for checkout target ordering and sequential behavior | S | D1 |
| D3. Add git integration regression test for sequential complete->start base preservation | S | D1 |
| D4. Add jj integration regression test for sequential complete->start base preservation | M | D1 |
| D5. Update architecture/workflow docs to reflect checkout target semantics (HEAD-first fallback) | S | D1 |

### Non-Goals (Explicit Exclusions)
- No change to how `start_commit` is recorded on `start()`.
- No change to bookmark naming or branch lifecycle policy.
- No change to `task_service` completion semantics, blocker graph, or learnings bubbling.
- No change to monorepo `repoPath` handling in host/mcp/ui.

### Data Model
No schema changes.

Affected persisted fields remain:
- `tasks.bookmark` (cleared only after successful VCS delete)
- `tasks.start_commit` (recorded at start; retained for fallback/history)
- `tasks.commit_sha` (recorded by completion service)

### API/Interface Contract
No public CLI/MCP signature changes.

Behavioral contract update (internal workflow semantics):
- `complete` and milestone `complete` cleanup now select checkout target as:
  1) `current_commit_id` (preferred), then
  2) stored `start_commit` fallback(s).

Error handling unchanged:
- checkout/delete remain best-effort cleanup with warnings,
- completion result remains success if cleanup fails after DB completion.

### Acceptance Criteria
- [ ] After completing task A with a real commit, starting task B records `start_commit` at task A post-complete tip (not A pre-start commit).
- [ ] Same behavior holds for both git and jj backends.
- [ ] Completion still checks out a safe target before deleting branch/bookmark (git delete-on-checked-out branch still avoided).
- [ ] Cleanup fallback still works when `current_commit_id` is unavailable (uses stored start commit path).
- [ ] Existing lifecycle guards/idempotency behavior in workflow service remains unchanged.
- [ ] No public API changes required for CLI/MCP callers.

### Test Strategy
| Layer | What | How |
|-------|------|-----|
| Unit | checkout target selection order and fallback chain | Add focused tests in `overseer/src/core/workflow_service.rs` with stateful mock VCS capturing checkout/delete call order and targets |
| Integration (git) | sequential complete->start preserves tip baseline | New `overseer/tests/workflow_git_integration_test.rs`: create repo+tasks, complete A with real change, start B, assert B `start_commit` equals current tip after A completion |
| Integration (jj) | same regression in jj backend | New `overseer/tests/workflow_jj_integration_test.rs` mirroring git flow with jj backend |
| Regression safety | existing workflow semantics | Run full `cargo test`; ensure existing workflow tests pass unchanged |

### Risks & Mitigations
| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Hidden dependency on rewind behavior in existing users | Low | Medium | Document behavior update in architecture/workflow docs; keep fallback chain |
| Backend-specific divergence (git vs jj commit-id semantics) | Medium | Medium | Require both git and jj integration tests in this fix |
| Flaky integration tests due environment | Medium | Low | Keep tests isolated in temp repos and deterministic commit setup |
| Cleanup behavior regressions | Low | Medium | Add unit call-order assertions for checkout-before-delete |

### Trade-offs Made
| Chose | Over | Because |
|-------|------|---------|
| HEAD-first checkout target | start_commit-first rewind | preserves sequential stacking progression while still detaching before delete |
| Keep start_commit fallback | removing fallback entirely | protects cleanup when head resolution fails |
| Separate bugfix PR | folding into monorepo PR | smaller review surface, clearer regression intent |

### Open Questions
- [ ] None

### Success Metrics
- Sequential workflow repro no longer forks next task from stale base commit.
- New regression tests fail before fix and pass after fix on both git/jj.
- No failures in existing workflow/unit/integration suites.
