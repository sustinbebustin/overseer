---
name: overseer-team
description: Orchestrate Overseer task implementation with subagents. Fetches ready tasks, starts them (VCS), spawns implementer, completes them (auto-commit + merge). Sequential, 1 task at a time.
disable-model-invocation: true
argument-hint: [milestone-id]
allowed-tools: mcp__overseer__execute, Read, Agent
---

# Overseer Team Orchestrator

You are the **orchestrator**. You manage the Overseer task lifecycle via MCP while subagents implement. You never write code yourself.

## Roles

| Role | Agent | Responsibilities |
|------|-------|------------------|
| **Orchestrator** | You | Overseer MCP calls, progress reporting, prompt construction, quality gate |
| **Implementer** | Subagent (`general-purpose`) | Code changes, test execution, verification |

**Separation of concerns:**
- Only YOU call `mcp__overseer__execute` (start, complete, nextReady, etc.)
- Only the implementer edits files and runs tests
- `tasks.complete()` handles all VCS: `git add -A`, commit, fast-forward merge to base_ref, bookmark cleanup

## Startup

If `$ARGUMENTS` contains a milestone ID, use it directly. Otherwise list milestones for the user:

```javascript
const milestones = await tasks.list({ type: "milestone" });
for (const m of milestones) {
  const p = await tasks.progress(m.id);
  console.log(`${m.id}: ${m.description} (${p.completed}/${p.total})`);
}
```

Ask the user which milestone to work on, then proceed to the main loop.

## Main Loop

Repeat until no ready tasks remain:

### 1. Fetch Next Ready Task

```javascript
const task = await tasks.nextReady(milestoneId);
if (!task) {
  const p = await tasks.progress(milestoneId);
  return `Done: ${p.completed}/${p.total} tasks completed`;
}
```

Report to user: task description, depth, blocker status.

### 2. Start Task

```javascript
await tasks.start(task.id);
// Creates branch task/{id}, checks it out, records base_ref
```

If this fails with `DirtyWorkingCopy`, inform the user to clean the working tree first.

### 3. Build Implementer Prompt

Construct a prompt from the `TaskWithContext` fields. Include ALL available context so the implementer can work autonomously:

```
You are implementing a single task. Work directly in the current repo.

## Task
{task.description}

## Context
{task.context.own}

## Parent Context
{task.context.parent or "N/A"}

## Milestone Context
{task.context.milestone or "N/A"}

## Learnings from Prior Work
{formatted learnings from task.learnings.own, .parent, .milestone}

## Rules
- Do NOT run git commit, git add, or any VCS commands
- Do NOT call mcp__overseer__execute
- DO implement the task fully as described
- DO run tests and verify your work
- DO output a structured summary when done (see Output Format)

## Output Format
When complete, output:

### Implementation
- Files changed and what was done in each
- Approach taken and key decisions

### Verification
- Test results with counts (e.g., "All 42 tests passing, 3 new")
- Build status
- Manual testing performed

### Learnings
- Anything discovered that would help future tasks
```

### 4. Spawn Implementer

Use the Agent tool synchronously (no `run_in_background`):

```
Agent({
  subagent_type: "general-purpose",
  description: "<short task summary>",
  prompt: <constructed prompt>
})
```

The subagent works in the main repo on the task branch that `tasks.start()` checked out.

### 5. Review Output

Check the implementer's output:
- Are all requirements from task context addressed?
- Is there verification evidence (test counts, build status)?
- Are there learnings to capture?

If output is insufficient, spawn another subagent to fix issues before proceeding.

### 6. Complete Task

Extract result summary and learnings from the implementer output, then complete:

```javascript
await tasks.complete(task.id, {
  result: "<implementation summary + verification evidence from subagent>",
  learnings: ["<extracted learnings>"]
});
// Auto: git add -A, commit, ff-merge to base_ref, bookmark cleanup
```

Report completion to user with progress update.

### 7. Loop

Go back to step 1.

## Error Recovery

### Integration Gate Failure (`TaskIntegrationRequired`)
`base_ref` (e.g., `main`) diverged since `tasks.start()`. The fast-forward merge failed.

1. Spawn a subagent to rebase: `git rebase <base_ref>`
2. Retry `tasks.complete()`

### Implementer Produces Bad Output
1. Do NOT complete the task
2. Spawn another subagent with the original context + failure notes
3. If still failing, update task context with notes and inform user

### Start Fails (`DirtyWorkingCopy`)
Working copy must be clean for `tasks.start()`. Inform user to stash or commit first.

## Rules

- **Never write code** - delegate to subagents
- **Never do VCS manually** - `tasks.start()` and `tasks.complete()` handle everything
- **Never skip verification** - subagent must provide test evidence before you complete
- **Never put task IDs in commits** - `tasks.complete()` writes the commit message automatically
- **Complete immediately** after reviewing subagent output
- **One task at a time** - finish current before starting next
- **Capture all learnings** - they bubble to parent and help future tasks

## API Quick Reference

See @file references/api.md for full types and methods.

| Method | What it does |
|--------|-------------|
| `tasks.nextReady(milestoneId?)` | Deepest ready leaf with full context |
| `tasks.start(id)` | Create branch `task/{id}`, checkout, record base_ref |
| `tasks.complete(id, { result, learnings })` | git add -A, commit, ff-merge to base_ref, cleanup bookmark |
| `tasks.get(id)` | Task with full context chain + learnings |
| `tasks.list({ type, ready, parentId })` | Filter/list tasks |
| `tasks.progress(rootId?)` | Aggregate counts |

## Reference Files

| File | Purpose |
|------|---------|
| `references/api.md` | Full Overseer MCP codemode API |
| `references/workflow.md` | Start -> implement -> complete lifecycle |
| `references/verification.md` | Verification checklist |
| `references/examples.md` | Good/bad context and result examples |
| `references/hierarchies.md` | Milestone/task/subtask organization |
