# MCP Agent Guide

Overseer MCP server provides a single `execute` tool using the **codemode pattern**: agents write JavaScript that executes server-side, only results return.

## Why Codemode?

Traditional MCP tools require one tool call per operation. Codemode lets agents compose complex workflows in a single execution:

**Traditional (multiple tool calls):**
```
1. create_task(...)
2. add_learning(...)
3. start_task(...)
```

**Codemode (single execution):**
```javascript
const task = await tasks.create({...});
await tasks.start(task.id);
// ... do work ...
await tasks.complete(task.id, { result: "Done", learnings: ["Key insight"] });
return task;
```

**Benefits:**
- Fewer round trips
- Agents handle TypeScript APIs better than tool schemas
- Compose operations naturally with JS control flow

## The `execute` Tool

Single tool that runs JavaScript in VM sandbox with `tasks` and `learnings` APIs.

**Input:** `code` parameter (JavaScript string)  
**Output:** Return value from code execution  
**Timeout:** 30 seconds  
**Truncation:** Outputs >50,000 chars truncated with preview

## Type Definitions

```typescript
// Task (from list/create/update/start/complete/reopen)
// Note: Does NOT include context chain or inherited learnings
interface Task {
  id: string;                   // ULID (task_01JQAZ...)
  parentId: string | null;
  description: string;
  priority: number;             // 0-2 (p0=highest, p1=default, p2=lowest)
  completed: boolean;
  completedAt: string | null;
  startedAt: string | null;
  createdAt: string;            // ISO 8601
  updatedAt: string;
  result: string | null;        // Completion notes
  commitSha: string | null;     // Auto-populated on complete
  depth: number;                // 0=milestone, 1=task, 2=subtask
  blockedBy?: string[];         // Blocking task IDs (omitted if empty)
  blocks?: string[];            // Tasks this blocks (omitted if empty)
  bookmark?: string;            // VCS bookmark name (if started)
  startCommit?: string;         // Commit SHA at start
  effectivelyBlocked: boolean;  // True if task OR ancestor has incomplete blockers
}

// TaskTree (from tree)
interface TaskTree {
  task: Task;
  children: TaskTree[];
}

// TaskProgress (from progress)
interface TaskProgress {
  total: number;
  completed: number;
  ready: number;     // !completed && !effectivelyBlocked
  blocked: number;   // !completed && effectivelyBlocked
}

// TaskType alias for depth filter
type TaskType = "milestone" | "task" | "subtask";

// TaskWithContext (from get/nextReady)
// Extends Task with context chain and inherited learnings
interface TaskWithContext extends Task {
  context: {                    // Inherited context chain
    own: string;
    parent?: string;            // If depth > 0
    milestone?: string;         // If depth > 1
  };
  learnings: {                  // Inherited learnings
    own: Learning[];            // Learnings attached to this task
    parent: Learning[];         // Parent's learnings (if depth > 0)
    milestone: Learning[];      // Milestone's learnings (if depth > 1)
  };
}

// Learning
interface Learning {
  id: string;                   // ULID (lrn_01JQAZ...)
  taskId: string;
  content: string;
  sourceTaskId: string | null;
  createdAt: string;
}
```

## API Reference

### `tasks` API

```javascript
// List tasks
tasks.list(filter?: {
  parentId?: string;
  ready?: boolean;      // No blockers, not completed
  completed?: boolean;
  depth?: 0 | 1 | 2;    // 0=milestones, 1=tasks, 2=subtasks
  type?: TaskType;      // Alias: "milestone"|"task"|"subtask" (mutually exclusive with depth)
}): Promise<Task[]>

// Get task with context
tasks.get(id: string): Promise<TaskWithContext>

// Create task
tasks.create(input: {
  description: string;
  context?: string;
  parentId?: string;          // Makes this a subtask
  priority?: 0 | 1 | 2;          // Default: 1 (p0=highest)
  blockedBy?: string[];       // Task IDs
}): Promise<Task>

// Update task
tasks.update(id: string, input: {
  description?: string;
  context?: string;
  priority?: 0 | 1 | 2;          // 0-2 (p0=highest)
  parentId?: string;
}): Promise<Task>

// State transitions
// Start follows blockers to find startable work, cascades to deepest leaf
tasks.start(id: string, options?: { repoPath?: string }): Promise<Task>
// Complete with optional result and learnings
// Learnings bubble to immediate parent, auto-bubbles up completion if all siblings done
tasks.complete(id: string, options?: { result?: string; learnings?: string[]; repoPath?: string }): Promise<Task>
tasks.reopen(id: string): Promise<Task>
tasks.delete(id: string): Promise<void>

// Blockers
tasks.block(taskId: string, blockerId: string): Promise<void>
tasks.unblock(taskId: string, blockerId: string): Promise<void>

// Queries - DFS to find deepest unblocked incomplete leaf
// Returns TaskWithContext (with context chain + learnings) or null
tasks.nextReady(milestoneId?: string): Promise<TaskWithContext | null>

// Tree - returns nested task structure
// If rootId provided, returns single tree; otherwise returns array of all milestone trees
tasks.tree(rootId?: string): Promise<TaskTree | TaskTree[]>

// Search - find tasks by description/context/result (case-insensitive)
tasks.search(query: string): Promise<Task[]>

// Progress - aggregate counts for a milestone or all tasks
tasks.progress(rootId?: string): Promise<TaskProgress>
```

### `learnings` API

```javascript
// List learnings for task
// Note: Learnings are added via tasks.complete({ learnings: [...] })
learnings.list(taskId: string): Promise<Learning[]>
```

## Usage Patterns

### Create Task Hierarchy

```javascript
// Create milestone
const milestone = await tasks.create({
  description: "Implement user authentication",
  context: "JWT-based auth with refresh tokens, bcrypt for passwords",
  priority: 0  // p0 = highest
});

// Create subtasks
const loginTask = await tasks.create({
  description: "Add login endpoint",
  parentId: milestone.id,
  priority: 0  // p0 = highest
});

const signupTask = await tasks.create({
  description: "Add signup endpoint", 
  parentId: milestone.id,
  priority: 1,  // p1 = default
  blockedBy: [loginTask.id]  // Blocked until login done
});

return { milestone, tasks: [loginTask, signupTask] };
```

### Start and Complete Task

```javascript
// Get next ready task (DFS finds deepest unblocked leaf)
const task = await tasks.nextReady();

if (!task) {
  return "No tasks ready";
}

// Start task - if blocked, follows blockers to find startable work
// Cascades to deepest incomplete leaf
await tasks.start(task.id);

// ... do work ...

// Complete with result and learnings (auto-captures commit SHA)
// Learnings bubble to immediate parent
// Auto-bubbles up: if all siblings done and parent unblocked,
// parent is auto-completed too
await tasks.complete(task.id, {
  result: "Login endpoint implemented with JWT tokens",
  learnings: ["bcrypt rounds should be 12 for production"]
});

return task;
```

### Progressive Context

```javascript
// Get subtask with inherited context
const subtask = await tasks.get(subtaskId);

// subtask.context contains:
// - own: subtask's context
// - parent: parent task's context  
// - milestone: root milestone's context

// subtask.learnings contains:
// - own: learnings attached directly to this task
// - parent: learnings from parent task
// - milestone: learnings from root

console.log("Milestone context:", subtask.context.milestone);
console.log("Own learnings:", subtask.learnings.own);
console.log("Parent learnings:", subtask.learnings.parent);
```

### VCS Integration (Required for Workflow)

VCS operations are integrated into task lifecycle - no manual VCS API calls needed:

```javascript
// Complete task - VCS required, commits changes
await tasks.complete(task.id, { result: "Login endpoint complete" });
// -> Commits changes (NothingToCommit treated as success)
// -> Stores commit SHA on task
```

**VCS is required** for `start` and `complete`. Fails with `NotARepository` if no jj/git found, `DirtyWorkingCopy` if uncommitted changes. In monorepos where workspace root is not a repo, pass `repoPath` on workflow calls (absolute or relative to host cwd). CRUD operations (create, list, get, etc.) work without VCS.

When provided, `repoPath` must exist and be a directory.

### Error Handling

```javascript
try {
  const task = await tasks.get("task_01JQAZ...");
  await tasks.complete(task.id);
} catch (err) {
  if (err.message.includes("pending children")) {
    // Task has incomplete subtasks
    const children = await tasks.list({ parentId: task.id, completed: false });
    return `Cannot complete: ${children.length} pending children`;
  }
  throw err;
}
```

### Batch Operations

```javascript
// Find and complete multiple tasks
const readyTasks = await tasks.list({ ready: true });

const completed = [];
for (const task of readyTasks) {
  if (task.description.includes("test")) {
    await tasks.start(task.id);
    await tasks.complete(task.id, { result: "Tests passing" });
    completed.push(task);
  }
}

return { completed: completed.length, tasks: completed };
```

### Search, Filter, and Progress

```javascript
// Get all completed tasks
const done = await tasks.list({ completed: true });

// Get only milestones (two equivalent ways)
const milestones = await tasks.list({ depth: 0 });
const milestones2 = await tasks.list({ type: "milestone" });

// Search tasks by text
const authTasks = await tasks.search("authentication");

// Get progress summary (much more efficient than counting manually)
const progress = await tasks.progress(milestoneId);
// -> { total: 10, completed: 3, ready: 5, blocked: 2 }

// Get task tree structure
const tree = await tasks.tree(milestoneId);
// -> { task: Task, children: TaskTree[] }

// Get all milestone trees
const allTrees = await tasks.tree();
// -> TaskTree[] (one per milestone)
```

## Best Practices

### 1. Use Progressive Context

Always fetch tasks with `tasks.get()` to access inherited context:

```javascript
// ✅ Good - gets full context
const task = await tasks.get(taskId);
console.log(task.context.milestone); // Root context

// ❌ Bad - no context
const tasks = await tasks.list({ parentId: milestoneId });
// tasks[0] has no inherited context
```

### 2. Capture Learnings on Complete

Include learnings when completing tasks:

```javascript
// ✅ Good - capture learnings on complete
await tasks.complete(taskId, {
  result: "Feature implemented",
  learnings: ["bcrypt default rounds too low"]
});

// Note: Learnings are added via tasks.complete(), not a separate API
```

### 3. Use Blockers for Dependencies

Explicit blockers prevent premature task start:

```javascript
// ✅ Good - explicit dependency
await tasks.create({
  description: "Deploy to prod",
  blockedBy: [testTaskId, reviewTaskId]
});

// ❌ Bad - implicit ordering
await tasks.create({ description: "Deploy to prod" });
// No guarantee tests/review done first
```

### 4. Return Useful Data

Return structured data for agent inspection:

```javascript
// ✅ Good - return summary
return {
  created: milestone.id,
  subtasks: tasks.length,
  nextReady: tasks.find(t => !t.blockedBy)?.id
};

// ❌ Bad - unclear result
return "Done";
```

## Common Patterns

### Task Breakdown

```javascript
const milestone = await tasks.create({
  description: "User auth system",
  context: "JWT + refresh tokens"
});

const subtasks = [
  "Add login endpoint",
  "Add signup endpoint", 
  "Add token refresh",
  "Add password reset"
];

for (const desc of subtasks) {
  await tasks.create({
    description: desc,
    parentId: milestone.id
  });
}

return await tasks.list({ parentId: milestone.id });
```

### Work Session

```javascript
// Get next task with full context
const task = await tasks.nextReady();
if (!task) return "No ready tasks";

// Review context
console.log("Milestone:", task.context.milestone);
console.log("Parent:", task.context.parent);
console.log("Task:", task.context.own);

// Check inherited learnings
console.log("Learnings:", task.learnings.parent);

// Start work (creates bookmark, records start commit)
await tasks.start(task.id);
return task;
```

### Complete Task (VCS Auto-Handled)

```javascript
// Complete task - VCS required, commits changes
const completed = await tasks.complete(taskId, {
  result: "Feature X implemented and tested",
  learnings: ["Key discovery during implementation"]
});

// completed.commitSha contains the commit SHA
return { task: completed, commit: completed.commitSha };
```

## Troubleshooting

### Task not ready?

```javascript
const task = await tasks.get(taskId);

// Check blockers
const blockers = await tasks.list({
  // Get blocker details by querying each blocked_by ID
});

// Unblock if needed
if (blockerCompleted) {
  await tasks.unblock(taskId, blockerId);
}
```

### Can't complete task?

```javascript
// Error: "pending children"
const children = await tasks.list({
  parentId: taskId,
  completed: false
});

console.log(`${children.length} children still pending`);
// Complete children first
```

## Limitations

- **Timeout:** 30s max execution
- **Output:** 50,000 chars max (larger outputs truncated)
- **No network:** Sandbox has no fetch/http access
- **No filesystem:** Cannot read/write files directly
- **VCS required for workflow:** `start` and `complete` require jj or git (fails with `NotARepository` error). CRUD operations work without VCS.

## Data Export

For backup or version control of task data, use the CLI `data` command:

```bash
# Export all tasks and learnings
os data export -o backup.json
```

This is a CLI-only command (not available via MCP execute tool). See [CLI Reference](CLI.md#data-management) for details.

## See Also

- [CLI Reference](CLI.md) - Direct `os` command usage
- [Architecture](ARCHITECTURE.md) - System design details
