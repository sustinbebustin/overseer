---
description: Finalize Overseer task work into a clean local branch (no PR)
argument-hint: "[target-branch] [--from <source-branch>] [--base <base-branch>] [--mode squash|preserve]"
allowed-tools: Bash(git *), Skill, AskUserQuestion
---

# /overseer-complete

Use this after implementation is done and you want clean local history.

Default behavior:
- `--base main`
- `--from` current branch
- `--mode squash` (recommended)
- **No PR creation**

## What this command does

1. Collect commits from `merge-base(base, source)..source`
2. Create a new local branch from `base`
3. Re-apply commits onto that branch:
   - `squash`: one final commit (best for messy task-completion commit messages)
   - `preserve`: keep original commit boundaries/messages
4. Verify final branch state and print summary

## Required workflow

1) **Load commit skill first**

```js
skill({ name: 'commit' })
```

2) **Parse args**

- `targetBranch` = first positional arg, or `local/<source>-final`
- `sourceBranch` = `--from` or current branch
- `baseBranch` = `--base` or `main`
- `mode` = `--mode` or `squash`

3) **Preflight checks**

- Run:
  - `git status --short`
  - `git rev-parse --abbrev-ref HEAD`
  - `git rev-parse --verify <baseBranch>`
  - `git rev-parse --verify <sourceBranch>`
- If working tree is dirty, ask user one question:
  - stash and continue (recommended), or
  - stop.
- If stash selected, run `git stash push -u -m "overseer-complete-temp"` and remember to pop later.

4) **Compute commit set**

- `BASE_SHA=$(git merge-base <baseBranch> <sourceBranch>)`
- `COMMITS=$(git rev-list --reverse "$BASE_SHA..<sourceBranch>")`
- If empty, stop and report no commits to migrate.

5) **Create target branch from base**

- If `targetBranch` exists, ask whether to:
  - use a new name (`<targetBranch>-v2`) (recommended), or
  - stop.
- Run:
  - `git checkout <baseBranch>`
  - `git checkout -b <targetBranch>`

6) **Apply history**

- If `mode=preserve`:
  - `git cherry-pick <commit1> <commit2> ...`
- If `mode=squash` (recommended):
  - `git cherry-pick -n <commit1> <commit2> ...`
  - create exactly one commit using conventional format:
    - run commit-skill workflow against staged diff
    - single atomic commit message, no AI attribution

7) **Verify result**

Run and report:
- `git status -sb`
- `git log --oneline --decorate -10`
- `git diff --stat <baseBranch>...HEAD`

8) **Restore stash if created**

- `git stash pop`
- If conflicts, report clearly and stop for manual resolution.

9) **Final output**

Return:
- target branch name
- source/base used
- mode used
- list of resulting commits on target branch
- confirmation: "No PR created"

## Notes

- This command is local-history cleanup only.
- `squash` is best when Overseer completion commits are noisy (e.g. `Complete: ...`).
- If you later want to publish, push the cleaned branch manually.
