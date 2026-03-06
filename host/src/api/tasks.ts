/**
 * Tasks API - typed wrapper around os task commands
 */
import { callCli } from "../cli.js";
import {
  decodeTask,
  decodeTasks,
  decodeTaskWithContext,
  decodeTaskWithContextOrNull,
  decodeTaskTree,
  decodeTaskTrees,
  decodeTaskProgress,
} from "../decoder.js";
import type { Depth, Priority, Task, TaskWithContext, TaskTree, TaskProgress } from "../types.js";

/**
 * Task type aliases for depth (ergonomic sugar)
 */
export type TaskType = "milestone" | "task" | "subtask";

const TYPE_TO_DEPTH: Record<TaskType, Depth> = {
  milestone: 0,
  task: 1,
  subtask: 2,
};

export interface TaskFilter {
  parentId?: string;
  ready?: boolean;
  completed?: boolean;
  /**
   * Filter by depth: 0=milestones, 1=tasks, 2=subtasks.
   * Maps to CLI: --milestones | --tasks | --subtasks
   * Mutually exclusive with parentId and type.
   */
  depth?: Depth;
  /**
   * Filter by type: "milestone" | "task" | "subtask".
   * Alias for depth (milestone=0, task=1, subtask=2).
   * Mutually exclusive with parentId and depth.
   */
  type?: TaskType;
  /**
   * Filter by archived state:
   * - undefined: hide archived (default)
   * - true: show only archived
   * - false: hide archived (explicit)
   * - "all": include all (archived and non-archived)
   */
  archived?: boolean | "all";
  /** Filter by repo path (exact match) */
  repoPath?: string;
}

export interface CreateTaskInput {
  description: string;
  context?: string;
  parentId?: string;
  priority?: Priority;
  blockedBy?: string[];
  /** Relative path from workspace root to repo (e.g. "frontend") */
  repoPath?: string;
}

export interface UpdateTaskInput {
  description?: string;
  context?: string;
  priority?: Priority;
  parentId?: string;
  /** Relative path from workspace root to repo */
  repoPath?: string;
}

export interface WorkflowRepoOptions {
  repoPath?: string;
}

/**
 * Tasks API exposed to VM sandbox
 */
export const tasks = {
  /**
   * List tasks with optional filters.
   * Returns tasks without context chain or inherited learnings.
   */
  async list(filter?: TaskFilter): Promise<Task[]> {
    // Resolve type to depth if provided
    const effectiveDepth = filter?.type !== undefined 
      ? TYPE_TO_DEPTH[filter.type] 
      : filter?.depth;

    if (filter?.parentId !== undefined && effectiveDepth !== undefined) {
      throw new Error(
        "parentId and depth/type are mutually exclusive - use parentId alone; depth is implied by parent type"
      );
    }
    if (filter?.depth !== undefined && filter?.type !== undefined) {
      throw new Error(
        "depth and type are mutually exclusive - use one or the other"
      );
    }

    const args = ["task", "list"];
    if (filter?.parentId) args.push("--parent", filter.parentId);
    if (filter?.ready) args.push("--ready");
    if (filter?.completed) args.push("--completed");
    if (effectiveDepth !== undefined) {
      const depthFlags: Record<Depth, string> = {
        0: "--milestones",
        1: "--tasks",
        2: "--subtasks",
      };
      args.push(depthFlags[effectiveDepth]);
    }
    // Archived filter
    if (filter?.archived === true) {
      args.push("--archived");
    } else if (filter?.archived === "all") {
      args.push("--all");
    }
    // Default (undefined or false) = hide archived (CLI default)
    if (filter?.repoPath) args.push("--repo", filter.repoPath);
    return decodeTasks(await callCli(args)).unwrap("tasks.list");
  },

  /**
   * Get single task with full context chain and inherited learnings.
   */
  async get(id: string): Promise<TaskWithContext> {
    return decodeTaskWithContext(await callCli(["task", "get", id])).unwrap("tasks.get");
  },

  /**
   * Create new task.
   * Returns task without context chain or inherited learnings.
   */
  async create(input: CreateTaskInput): Promise<Task> {
    const args = ["task", "create", "-d", input.description];
    if (input.context) args.push("--context", input.context);
    if (input.parentId) args.push("--parent", input.parentId);
    if (input.priority !== undefined) args.push("--priority", String(input.priority));
    if (input.blockedBy && input.blockedBy.length > 0) {
      args.push("--blocked-by", input.blockedBy.join(","));
    }
    if (input.repoPath) args.push("--repo", input.repoPath);
    return decodeTask(await callCli(args)).unwrap("tasks.create");
  },

  /**
   * Update existing task.
   * Returns task without context chain or inherited learnings.
   */
  async update(id: string, input: UpdateTaskInput): Promise<Task> {
    const args = ["task", "update", id];
    if (input.description) args.push("-d", input.description);
    if (input.context) args.push("--context", input.context);
    if (input.priority !== undefined) args.push("--priority", String(input.priority));
    if (input.parentId) args.push("--parent", input.parentId);
    if (input.repoPath) args.push("--repo", input.repoPath);
    return decodeTask(await callCli(args)).unwrap("tasks.update");
  },

  /**
   * Mark task as started.
   * Follows blockers to find startable work, cascades to deepest leaf.
   * Creates VCS bookmark for started task and records start commit.
   * Returns the task that was actually started.
   *
   * **Requires VCS**: Must be in a jj or git repository.
   */
  async start(id: string, options?: WorkflowRepoOptions): Promise<Task> {
    return decodeTask(await callCli(["task", "start", id], { cwd: options?.repoPath })).unwrap(
      "tasks.start"
    );
  },

  /**
   * Complete task with optional result and learnings.
   * Learnings are attached to the task and bubbled to immediate parent.
   * Auto-bubbles up if all siblings done and parent unblocked.
   * Commits changes and captures commit SHA.
   *
   * **Requires VCS**: Must be in a jj or git repository.
   */
  async complete(
    id: string,
    options?: { result?: string; learnings?: string[]; repoPath?: string }
  ): Promise<Task> {
    const args = ["task", "complete", id];
    if (options?.result) args.push("--result", options.result);
    if (options?.learnings) {
      for (const learning of options.learnings) {
        args.push("--learning", learning);
      }
    }
    return decodeTask(await callCli(args, { cwd: options?.repoPath })).unwrap("tasks.complete");
  },

  /**
   * Reopen completed task.
   */
  async reopen(id: string): Promise<Task> {
    return decodeTask(await callCli(["task", "reopen", id])).unwrap("tasks.reopen");
  },

  /**
   * Cancel a pending or in-progress task.
   * Cannot cancel completed or archived tasks.
   * Cannot cancel tasks with pending children.
   */
  async cancel(id: string): Promise<Task> {
    return decodeTask(await callCli(["task", "cancel", id])).unwrap("tasks.cancel");
  },

  /**
   * Archive a completed or cancelled task.
   * Archived tasks are hidden from default list views.
   * Cannot archive active (pending/in-progress) tasks.
   */
  async archive(id: string): Promise<Task> {
    return decodeTask(await callCli(["task", "archive", id])).unwrap("tasks.archive");
  },

  /**
   * Delete task (cascades to children and learnings).
   */
  async delete(id: string): Promise<void> {
    await callCli(["task", "delete", id]);
  },

  /**
   * Add blocker relationship.
   * Validates: no self-blocks, no ancestor/descendant blocks, no cycles.
   */
  async block(taskId: string, blockerId: string): Promise<void> {
    await callCli(["task", "block", taskId, "--by", blockerId]);
  },

  /**
   * Remove blocker relationship.
   */
  async unblock(taskId: string, blockerId: string): Promise<void> {
    await callCli(["task", "unblock", taskId, "--by", blockerId]);
  },

  /**
   * Get next ready task (DFS to find deepest unblocked incomplete leaf).
   * Returns task with full context chain and inherited learnings, or null if no ready tasks.
   */
  async nextReady(milestoneId?: string): Promise<TaskWithContext | null> {
    const args = ["task", "next-ready"];
    if (milestoneId) args.push("--milestone", milestoneId);
    return decodeTaskWithContextOrNull(await callCli(args)).unwrap("tasks.nextReady");
  },

  /**
   * Get task tree structure.
   * If rootId provided, returns single tree rooted at that task.
   * If no rootId, returns array of all milestone trees.
   *
   * **Warning:** Large trees may hit 50k output limit. Prefer scoping to specific milestone.
   */
  async tree(rootId?: string): Promise<TaskTree | TaskTree[]> {
    const args = ["task", "tree"];
    if (rootId) {
      args.push(rootId);
      return decodeTaskTree(await callCli(args)).unwrap("tasks.tree");
    }
    return decodeTaskTrees(await callCli(args)).unwrap("tasks.tree");
  },

  /**
   * Search tasks by description.
   * Returns tasks matching the query (case-insensitive substring match).
   */
  async search(query: string): Promise<Task[]> {
    return decodeTasks(await callCli(["task", "search", query])).unwrap("tasks.search");
  },

  /**
   * Get progress summary for a milestone or all tasks.
   * Returns aggregate counts: { total, completed, ready, blocked }
   */
  async progress(rootId?: string): Promise<TaskProgress> {
    const args = ["task", "progress"];
    if (rootId) args.push(rootId);
    return decodeTaskProgress(await callCli(args)).unwrap("tasks.progress");
  },
};
