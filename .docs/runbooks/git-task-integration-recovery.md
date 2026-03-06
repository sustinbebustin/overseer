# Git Task Integration Recovery Runbook

Use this when `tasks.complete()` fails integration checks in git mode.

## 1) Fast-forward failure recovery

### Symptom
- `tasks.complete()` fails with integration-required error.

### Goal
- Rebase task branch onto base branch, then retry completion.

### Steps
```bash
# Replace placeholders
TASK_BRANCH="task/<task-id>"
BASE_BRANCH="<base-branch>"

# Inspect divergence
git fetch --all --prune
git log --oneline --decorate --graph "${BASE_BRANCH}..${TASK_BRANCH}"
git log --oneline --decorate --graph "${TASK_BRANCH}..${BASE_BRANCH}"

# Rebase task branch on top of base
git checkout "${TASK_BRANCH}"
git rebase "${BASE_BRANCH}"

# Resolve conflicts if prompted
git status
git add -A
git rebase --continue

# Retry completion from your agent flow
# await tasks.complete(taskId, ...)
```

### Verify
```bash
git merge-base --is-ancestor "${BASE_BRANCH}" "${TASK_BRANCH}"
echo $?
```
- `0` means task branch is rebased on base branch tip history.

## 2) Legacy missing `base_ref` recovery

### Symptom
- `tasks.complete()` fails with missing-base-ref error.

### Goal
- Backfill `baseRef` using idempotent `tasks.start()` from intended base branch.

### Steps
```bash
BASE_BRANCH="<intended-base-branch>"
TASK_BRANCH="task/<task-id>"

# Ensure clean working copy
git status --porcelain

# Checkout intended base branch first
git checkout "${BASE_BRANCH}"

# Run idempotent start from agent (backfills baseRef when task already started)
# await tasks.start(taskId)

# Confirm task branch still exists
git branch --list "${TASK_BRANCH}"

# Retry completion
# await tasks.complete(taskId, ...)
```

### Verify
- `tasks.get(taskId)` shows `baseRef` populated.

## 3) Recover already-orphaned historical commits

### Symptom
- You have known SHAs or suspect dangling commits after branch deletion.

### Goal
- Recreate refs for dangling commits and integrate safely.

### Steps
```bash
# List dangling commits
git fsck --lost-found --no-reflogs

# Recreate branch refs for candidate SHAs
git branch "recover/<label-1>" "<sha1>"
git branch "recover/<label-2>" "<sha2>"

# Inspect recovered history
git log --oneline --decorate --graph "recover/<label-1>"

# Integrate into target branch via fast-forward/rebase/cherry-pick as needed
TARGET_BRANCH="<target-branch>"
git checkout "${TARGET_BRANCH}"
git cherry-pick "<sha1>"
```

### Verify reachability
```bash
git branch --contains "<sha1>"
git fsck --no-reflogs
```
- Recovered SHAs should be reachable from named branches.

## Notes
- Do not force-delete task branches during recovery.
- Keep branch deletion to safe `git branch -d` after integration succeeds.
