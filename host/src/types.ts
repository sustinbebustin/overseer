/**
 * Core domain types mirroring the Rust CLI output
 */

// Branded types for type-safe IDs
declare const TaskIdBrand: unique symbol;
declare const LearningIdBrand: unique symbol;

export type TaskId = string & { readonly [TaskIdBrand]: never };
export type LearningId = string & { readonly [LearningIdBrand]: never };

// Validation helpers
export function isTaskId(s: string): s is TaskId {
  return s.startsWith("task_") && s.length === 31; // "task_" + 26 ULID chars
}

export function isLearningId(s: string): s is LearningId {
  return s.startsWith("lrn_") && s.length === 30; // "lrn_" + 26 ULID chars
}

export function parseTaskId(s: string): TaskId {
  if (!isTaskId(s)) {
    throw new Error(`Invalid TaskId: ${s}`);
  }
  return s;
}

export function parseLearningId(s: string): LearningId {
  if (!isLearningId(s)) {
    throw new Error(`Invalid LearningId: ${s}`);
  }
  return s;
}

/**
 * Task context chain (inherited from hierarchy)
 */
export interface TaskContext {
  own: string;
  parent?: string;
  milestone?: string;
}

/**
 * Inherited learnings (own task + ancestors)
 */
export interface InheritedLearnings {
  /** Learnings attached directly to this task (bubbled from completed children) */
  own: Learning[];
  /** Learnings from parent task (depth > 0) */
  parent: Learning[];
  /** Learnings from root milestone (depth > 1) */
  milestone: Learning[];
}

/**
 * Priority levels: p0=highest, p1=default, p2=lowest
 */
export type Priority = 0 | 1 | 2;

/**
 * Task depth (0=milestone, 1=task, 2=subtask)
 */
export type Depth = 0 | 1 | 2;

/**
 * Task returned from list/create/update/start/complete/reopen
 * Does NOT include context chain or inherited learnings
 */
export interface Task {
  id: TaskId;
  parentId: TaskId | null;
  description: string;
  priority: Priority;
  completed: boolean;
  completedAt: string | null;
  startedAt: string | null;
  createdAt: string;
  updatedAt: string;
  result: string | null;
  commitSha: string | null;
  depth: Depth;
  blockedBy?: TaskId[];
  blocks?: TaskId[];
  bookmark?: string;
  startCommit?: string;
  baseRef?: string;
  /** Computed: true if task or any ancestor has incomplete blockers */
  effectivelyBlocked: boolean;
  /** Task was cancelled (abandoned without completion) */
  cancelled: boolean;
  /** Timestamp when task was cancelled */
  cancelledAt: string | null;
  /** Task is archived (hidden from default views) */
  archived: boolean;
  /** Timestamp when task was archived */
  archivedAt: string | null;
}

/**
 * Task returned from get/nextReady - includes context chain and inherited learnings
 */
export interface TaskWithContext extends Task {
  context: TaskContext;
  learnings: InheritedLearnings;
}

export interface Learning {
  id: LearningId;
  taskId: TaskId;
  content: string;
  sourceTaskId: TaskId | null;
  createdAt: string;
}

/**
 * Recursive task tree node (from os task tree)
 */
export interface TaskTree {
  task: Task;
  children: TaskTree[];
}

/**
 * Progress summary (aggregate counts)
 */
export interface TaskProgress {
  total: number;
  completed: number;
  ready: number;     // !completed && !effectivelyBlocked
  blocked: number;   // !completed && effectivelyBlocked
}

/**
 * CLI command errors
 */
export class CliError extends Error {
  constructor(
    message: string,
    public exitCode: number,
    public stderr: string
  ) {
    super(message);
    this.name = "CliError";
  }
}

export class CliTimeoutError extends Error {
  constructor(message = "CLI command timeout (30s)") {
    super(message);
    this.name = "CliTimeoutError";
  }
}

export class InvalidPathError extends Error {
  constructor(
    message: string,
    public pathValue: string
  ) {
    super(message);
    this.name = "InvalidPathError";
  }
}
