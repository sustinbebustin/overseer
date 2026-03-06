# Feature: High/medium review findings remediation

### Problem Statement
**Who:** agents and users running Overseer across multi-repo workspaces (for example `app/frontend` + `app/backend`) and automation that relies on strict task/workflow invariants.  
**What:** current uncommitted changes introduce correctness gaps in workflow auto-completion, workspace root resolution, export integrity, commit SHA provenance, optional-string argument handling, and `repoPath` clearing.  
**Why it matters:** these paths are core lifecycle and data-integrity surfaces; regressions here cause failed workflow operations, inconsistent task state, and silent data loss risk in exports.  
**Evidence:** oracle-validated findings A1/A2/A3/B1/B2/B3/C1 from the latest uncommitted diff review.

### Proposed Solution
Implement one remediation slice focused on high+medium findings only. Keep schema stable, preserve existing external behavior where possible, and introduce explicit APIs only when ambiguity exists.

The key architectural choice is to decouple workflow workspace resolution from DB path location and make workspace/root semantics explicit for multi-repo operation: task `repoPath` stays relative to workspace root; cross-repo milestones keep `repoPath` unset. This supports the `app` root model where `.overseer` lives at root and child repos live below it.

For mutation contracts, prefer explicit intent over overloaded optional fields. Add an explicit repo-clear operation (`--clear-repo` / `clearRepoPath`) instead of tri-state nullable strings.

### Scope & Deliverables
| Deliverable | Effort | Depends On |
|-------------|--------|------------|
| D1. Enforce workflow invariants during bubble-up parent completion (`A2`) | M | - |
| D2. Source `commitSha` from workflow VCS backend, not process CWD (`A3`) | M | D1 |
| D3. Replace DB-path-derived workspace root with invocation-context resolver (`A1`) | M | - |
| D4. Make `data export` deterministic and fail-safe (no silent task drops) (`C1`) | S | - |
| D5. Fix host/UI optional string arg handling (`B1` + `B2`) | S | - |
| D6. Add explicit `repoPath` clear contract (`B3`) across CLI + host API + Rust service/repo | M | - |
| D7. Add focused tests for D1-D6 and update changed contracts in docs | M | D1-D6 |

### Non-Goals (Explicit Exclusions)
- Low-severity-only findings from this review round (for example docs wording-only mismatches not required by D7 changes).
- Schema migrations unrelated to these findings.
- Reworking full workflow API semantics beyond required bug fixes.
- Large refactors in UI client behavior outside touched route contract fixes.

### Data Model
- No DB schema migration required.
- `UpdateTaskInput` gains explicit clear intent:
  - add `clear_repo_path: bool` in Rust domain model.
  - keep `repo_path: Option<String>` for set/update.
- Task completion persistence path adds explicit commit SHA plumbing from workflow layer:
  - add TaskService completion entrypoint that accepts `commit_sha: Option<&str>` (or equivalent internal helper).
  - existing completion API remains as wrapper for non-workflow callers.

### API/Interface Contract
**CLI**
- `os task update TASK_ID [--repo <REL_PATH> | --clear-repo]`
- `--repo` and `--clear-repo` are mutually exclusive.
- Clear is rejected for started tasks with existing guard semantics (`InvalidRepoPath` reason unchanged except message text as needed).

**Host API (`host/src/api/tasks.ts`)**
- `UpdateTaskInput` adds `clearRepoPath?: boolean`.
- Reject conflicting input (`repoPath` + `clearRepoPath`) at boundary.
- Optional strings (`description`, `context`, `result`) use `!== undefined` checks so `""` is preserved when intentionally provided.

**UI API routes (`ui/src/api/routes/tasks.ts`)**
- Preserve empty string for update/complete fields using `!== undefined` checks.

**Workspace/root resolution (`overseer/src/main.rs`)**
- Replace `workspace_root_from_db(db_path)` derivation with resolver based on invocation context:
  1. start from process CWD,
  2. walk ancestors for `.overseer` workspace marker,
  3. fallback to VCS root detection,
  4. final fallback to CWD.
- DB path becomes independent from workflow workspace root.

**Export behavior (`overseer/src/commands/data.rs`)**
- `os data export` must not silently skip tasks on read errors.
- Export either succeeds with complete dataset or returns error.

### Acceptance Criteria
- [ ] Bubble-up auto-completion for non-milestone parents uses workflow completion path (same VCS/integration invariants as explicit complete).
- [ ] No auto-complete path bypasses git integration gate for git tasks with bookmark/baseRef.
- [ ] Workflow-completed tasks record `commitSha` from the same VCS backend used for completion, not process CWD.
- [ ] Workflow/delete commands no longer derive workspace root from DB file location.
- [ ] In `app` root with child repos (`frontend`, `backend`), tasks with `repoPath` resolve to `app/<repoPath>` regardless of DB override path.
- [ ] `data export` never silently drops tasks due to swallowed read errors.
- [ ] Host/UI preserve intentionally empty `description/context/result` values instead of silently omitting args.
- [ ] Users can clear `repoPath` explicitly (`--clear-repo` / `clearRepoPath`) on non-started tasks.
- [ ] Clearing or changing `repoPath` after start remains rejected.
- [ ] All touched tests pass in Rust + host + UI typecheck suites.

### Test Strategy
| Layer | What | How |
|-------|------|-----|
| Rust unit (`workflow_service`) | Bubble-up parent completion path, git gate enforcement, commit SHA plumbing | Add/adjust tests in `overseer/src/core/workflow_service.rs` using existing git test repo harness |
| Rust unit (`main.rs`) | Workspace root resolution from invocation context | Add focused tests for resolver edge paths (ancestor `.overseer`, VCS fallback, plain cwd fallback) |
| Rust unit (`commands/data.rs`) | Export fail behavior and no silent drops | Add tests asserting export errors on simulated read failures and success path completeness |
| Rust unit/integration (`task_service` + repo) | `--clear-repo` semantics and started-task guard | Add tests in `overseer/tests/task_service_test.rs` and/or command handler tests |
| Host TS | Arg building for empty strings + clearRepoPath conflict checks | Add small tests (or boundary assertions) in `host` package for `tasks.update/complete` argument construction |
| UI TS | Route arg handling for empty strings | Add route-level tests or equivalent request-handler assertions for `PUT /tasks/:id` and `POST /tasks/:id/complete` |

### Risks & Mitigations
| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| Bubble-up now enforces stricter workflow checks and may expose latent parent-task VCS issues | Medium | Medium | Add targeted tests for started-parent and blocked-parent cases; document expected failure mode |
| Workspace root resolver ambiguity in unusual custom layouts | Medium | Medium | Deterministic resolver order + tests for `app` multi-repo shape + fallback behavior |
| New clear contract drifts between Rust and host API | Medium | Medium | Single explicit flag in CLI + mirrored host type + boundary conflict checks |
| Export behavior change can break scripts expecting best-effort partial output | Low | Medium | Document fail-fast semantics in CLI docs; keep output schema stable |

### Trade-offs Made
| Chose | Over | Because |
|-------|------|---------|
| Explicit `--clear-repo` / `clearRepoPath` | Tri-state nullable `repoPath` | Lower cross-layer type churn, clearer intent for agents |
| Invocation-context workspace resolver | DB-path-derived workspace root | Correct for DB overrides and monorepo/root-tooling layouts |
| Fail-fast export | Silent best-effort export | Backup integrity > partial-success ambiguity |
| Explicit commit SHA plumbing from workflow | CWD-based SHA inference in TaskService | Correct provenance in multi-repo execution |

### Implementation Notes (Likely Files)
- `overseer/src/core/workflow_service.rs`
- `overseer/src/core/task_service.rs`
- `overseer/src/main.rs`
- `overseer/src/commands/data.rs`
- `overseer/src/commands/task.rs`
- `overseer/src/types.rs`
- `overseer/src/db/task_repo.rs`
- `host/src/api/tasks.ts`
- `host/src/types.ts`
- `host/src/mcp.ts` (if API docs surface `clearRepoPath`)
- `ui/src/api/routes/tasks.ts`
- `docs/CLI.md` (only changed contracts)

### Success Metrics
- High+medium oracle-validated findings in this scope are closed with tests.
- No regression in start/complete/delete workflows for canonical `app` multi-repo layout.
- Export path either succeeds fully or fails explicitly.

### Open Questions
- [ ] None
